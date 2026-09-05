use super::{AcpConnection, AcpError, AcpSession};
use crate::{
    ConfigChoice, ConfigOption, ConfigValue, ConfigValues, RunConfiguration, SessionConfiguration,
};
use agent_client_protocol::{
    JsonRpcRequest,
    schema::v1::{
        NewSessionResponse, SessionConfigKind, SessionConfigOption, SessionConfigOptionValue,
        SessionConfigSelectOptions, SetSessionConfigOptionRequest,
    },
};
use std::{
    collections::HashSet,
    sync::{Arc, Mutex},
};
use tokio::{sync::oneshot, time::timeout};

#[derive(Default)]
pub(super) struct ConfigurationState {
    options: Option<Vec<ConfigOption>>,
    requested: ConfigValues,
    pending: bool,
    uncertain: bool,
}

impl ConfigurationState {
    fn new(options: Option<&[SessionConfigOption]>) -> Result<Self, AcpError> {
        Ok(Self {
            options: options.map(normalize).transpose()?,
            ..Self::default()
        })
    }

    pub(super) fn observe(&mut self, options: &[SessionConfigOption]) {
        match normalize(options) {
            Ok(options) => {
                self.options = Some(options);
                self.uncertain = false;
            }
            Err(_) => self.uncertain = true,
        }
    }

    pub(super) fn snapshot(&self) -> SessionConfiguration {
        SessionConfiguration {
            options: self.options.clone(),
            pending: self.pending,
            uncertain: self.uncertain,
            values: RunConfiguration {
                requested: self.requested.clone(),
                confirmed: if self.pending || self.uncertain {
                    None
                } else {
                    self.options.as_ref().map(|options| {
                        options
                            .iter()
                            .map(|o| (o.id.clone(), o.current.clone()))
                            .collect()
                    })
                },
            },
        }
    }

    pub(super) fn for_run(&self) -> Result<RunConfiguration, AcpError> {
        if self.pending || self.uncertain {
            return Err(AcpError::ConfigurationUncertain);
        }
        Ok(RunConfiguration {
            requested: self.requested.clone(),
            confirmed: self.options.as_ref().map(|options| {
                options
                    .iter()
                    .map(|o| (o.id.clone(), o.current.clone()))
                    .collect()
            }),
        })
    }
}

pub(super) fn normalize(options: &[SessionConfigOption]) -> Result<Vec<ConfigOption>, AcpError> {
    let mut ids = HashSet::new();
    options.iter().map(|option| {
        let id = option.id.to_string();
        if id.trim().is_empty() || !ids.insert(id.clone()) { return Err(AcpError::InvalidConfiguration); }
        let (current, choices) = match &option.kind {
            SessionConfigKind::Boolean(value) => (ConfigValue::Boolean(value.current_value), vec![]),
            SessionConfigKind::Select(value) => {
                let mut choices = Vec::new();
                let mut add = |options: &[agent_client_protocol::schema::v1::SessionConfigSelectOption], group: Option<&str>| {
                    choices.extend(options.iter().map(|o| ConfigChoice {
                        value: o.value.to_string(), label: o.name.clone(), description: o.description.clone(), group: group.map(str::to_owned),
                    }));
                };
                match &value.options {
                    SessionConfigSelectOptions::Ungrouped(options) => add(options, None),
                    SessionConfigSelectOptions::Grouped(groups) => for group in groups { add(&group.options, Some(&group.name)); },
                    _ => return Err(AcpError::InvalidConfiguration),
                }
                let mut values = HashSet::new();
                if choices.iter().any(|o| o.value.trim().is_empty() || !values.insert(o.value.clone())) { return Err(AcpError::InvalidConfiguration); }
                (ConfigValue::Select(value.current_value.to_string()), choices)
            }
            _ => return Err(AcpError::InvalidConfiguration),
        };
        Ok(ConfigOption { id, label: option.name.clone(), description: option.description.clone(),
            category: option.category.as_ref().and_then(|c| serde_json::to_value(c).ok()).and_then(|v| v.as_str().map(str::to_owned)),
            current, choices })
    }).collect()
}

impl AcpConnection {
    /// Install the catalog in dispatch order, before updates following a setup
    /// response can arrive. Native SDK details stay behind this adapter.
    pub(super) async fn setup_configuration<R>(
        &self,
        request: R,
        convert: impl FnOnce(R::Response) -> NewSessionResponse + Send + 'static,
    ) -> Result<(NewSessionResponse, Arc<Mutex<ConfigurationState>>), AcpError>
    where
        R: JsonRpcRequest,
        R::Response: Send + 'static,
    {
        let routes = self.routes.clone();
        let (tx, rx) = oneshot::channel();
        self.connection
            .send_request(request)
            .on_receiving_result(move |result| async move {
                let initialized = result.map_err(AcpError::Protocol).and_then(|response| {
                    let info = convert(response);
                    let configuration = Arc::new(Mutex::new(ConfigurationState::new(
                        info.config_options.as_deref(),
                    )?));
                    let mut routes = routes.lock().unwrap();
                    if routes
                        .configurations
                        .get(&info.session_id)
                        .and_then(std::sync::Weak::upgrade)
                        .is_some()
                    {
                        return Err(AcpError::SessionUnavailable);
                    }
                    routes
                        .configurations
                        .insert(info.session_id.clone(), Arc::downgrade(&configuration));
                    Ok((info, configuration))
                });
                let _ = tx.send(initialized);
                Ok(())
            })
            .map_err(AcpError::Protocol)?;
        timeout(self.session_timeout, rx)
            .await
            .map_err(|_| AcpError::RequestTimedOut)?
            .map_err(|_| AcpError::Closed)?
    }
}

impl AcpSession<'_> {
    /// Current offered options and provider reports. Defaults are never invented.
    pub fn configuration(&self) -> SessionConfiguration {
        self.configuration.lock().unwrap().snapshot()
    }

    /// Apply a single validated option between runs. Dependent settings may change;
    /// the returned snapshot contains the provider's complete reported catalog.
    pub async fn set_option(
        &mut self,
        id: &str,
        value: ConfigValue,
    ) -> Result<SessionConfiguration, AcpError> {
        if self.retired || !self.quiescent {
            return Err(AcpError::SessionUnavailable);
        }
        if self.connection.is_closed() {
            return Err(AcpError::Closed);
        }
        {
            let mut state = self.configuration.lock().unwrap();
            if state.pending {
                return Err(AcpError::ConfigurationUncertain);
            }
            let catalog = state
                .options
                .as_ref()
                .ok_or(AcpError::ConfigurationUnsupported)?;
            let option = catalog
                .iter()
                .find(|o| o.id == id)
                .ok_or(AcpError::UnknownConfigurationOption)?;
            let valid = match (&option.current, &value) {
                (ConfigValue::Boolean(_), ConfigValue::Boolean(_)) => true,
                (ConfigValue::Select(_), ConfigValue::Select(value)) => {
                    option.choices.iter().any(|o| &o.value == value)
                }
                _ => false,
            };
            if !valid {
                return Err(AcpError::InvalidConfigurationValue);
            }
            state.pending = true;
        }
        let native_value = match &value {
            ConfigValue::Select(value) => SessionConfigOptionValue::value_id(value.clone()),
            ConfigValue::Boolean(value) => SessionConfigOptionValue::Boolean { value: *value },
        };
        let state = self.configuration.clone();
        let id = id.to_owned();
        let (tx, rx) = oneshot::channel();
        let request = SetSessionConfigOptionRequest::new(
            self.info.session_id.clone(),
            id.clone(),
            native_value,
        );
        let sent = self
            .connection
            .connection
            .send_request(request)
            .on_receiving_result(move |result| async move {
                let mut state = state.lock().unwrap();
                state.pending = false;
                state.uncertain = true;
                let result = result.map_err(AcpError::Protocol).and_then(|response| {
                    let options = normalize(&response.config_options)?;
                    let accepted = options.iter().any(|o| o.id == id && o.current == value);
                    state.options = Some(options);
                    state.uncertain = false;
                    if !accepted {
                        return Err(AcpError::ConfigurationRejected);
                    }
                    state.requested.insert(id, value);
                    Ok(state.snapshot())
                });
                let _ = tx.send(result);
                Ok(())
            });
        if let Err(error) = sent {
            let mut state = self.configuration.lock().unwrap();
            state.pending = false;
            state.uncertain = true;
            return Err(AcpError::Protocol(error));
        }
        match timeout(self.connection.session_timeout, rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => {
                self.configuration.lock().unwrap().uncertain = true;
                Err(AcpError::Closed)
            }
            Err(_) => {
                self.configuration.lock().unwrap().uncertain = true;
                Err(AcpError::RequestTimedOut)
            }
        }
    }

    /// Convenience for providers exposing exactly one model-category selector.
    /// Use set_option for absent or ambiguous categories.
    pub async fn set_model(
        &mut self,
        model: impl Into<String>,
    ) -> Result<SessionConfiguration, AcpError> {
        let options = self
            .configuration()
            .options
            .ok_or(AcpError::ConfigurationUnsupported)?;
        let models: Vec<_> = options
            .iter()
            .filter(|o| o.category.as_deref() == Some("model"))
            .collect();
        if models.len() != 1 {
            return Err(AcpError::ModelSelectorUnavailable);
        }
        self.set_option(&models[0].id, ConfigValue::Select(model.into()))
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn option(id: &str) -> SessionConfigOption {
        serde_json::from_value(
            json!({"id":id,"name":"Model","type":"select","category":"model",
            "currentValue":"a","options":[{"value":"a","name":"A"},{"value":"b","name":"B"}]}),
        )
        .unwrap()
    }

    #[test]
    fn invalid_provider_reports_do_not_leave_stale_confirmation_usable() {
        let mut state = ConfigurationState::new(Some(&[option("model")])).unwrap();
        assert!(state.for_run().unwrap().confirmed.is_some());
        state.observe(&[option("model"), option("model")]);
        assert!(state.snapshot().uncertain);
        assert!(state.snapshot().values.confirmed.is_none());
        assert!(matches!(
            state.for_run(),
            Err(AcpError::ConfigurationUncertain)
        ));
        state.observe(&[option("model")]);
        assert!(state.for_run().unwrap().confirmed.is_some());
    }

    #[test]
    fn duplicate_choices_and_blank_option_ids_are_rejected() {
        assert!(matches!(
            normalize(&[option(" ")]),
            Err(AcpError::InvalidConfiguration)
        ));
        let duplicated: SessionConfigOption = serde_json::from_value(json!({"id":"model","name":"Model","type":"select",
            "currentValue":"a","options":[{"value":"a","name":"A"},{"value":"a","name":"Duplicate"}]})).unwrap();
        assert!(matches!(
            normalize(&[duplicated]),
            Err(AcpError::InvalidConfiguration)
        ));
    }
}
