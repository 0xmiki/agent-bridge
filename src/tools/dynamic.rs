use super::*;

// Initial interoperable subset. Reject unsupported assertions instead of
// advertising semantics that this bridge has not tested. No remote references.
fn check_schema(schema: &Value, depth: usize, nodes: &mut usize) -> Result<(), ToolError> {
    *nodes += 1;
    if depth > 16 || *nodes > 256 {
        return Err(ToolError::InvalidSchema(
            "schema exceeds node/depth limits".into(),
        ));
    }
    if schema.is_boolean() {
        return Ok(());
    }
    let object = schema.as_object().ok_or(ToolError::InvalidDefinition)?;
    for (key, value) in object {
        match key.as_str() {
            "$schema" if value == "https://json-schema.org/draft/2020-12/schema" => {}
            "type" | "title" | "description" | "enum" | "const" | "required" | "minLength"
            | "maxLength" | "minimum" | "maximum" | "exclusiveMinimum" | "exclusiveMaximum"
            | "minItems" | "maxItems" | "uniqueItems" | "minProperties" | "maxProperties" => {}
            "properties" => {
                for schema in value
                    .as_object()
                    .ok_or(ToolError::InvalidDefinition)?
                    .values()
                {
                    check_schema(schema, depth + 1, nodes)?;
                }
            }
            "items" | "additionalProperties" => check_schema(value, depth + 1, nodes)?,
            _ => {
                return Err(ToolError::InvalidSchema(format!(
                    "unsupported schema keyword: {key}"
                )));
            }
        }
    }
    Ok(())
}

/// Compile the bounded, local-only schema subset shared by tools and hosted results.
pub fn compile_schema(schema: &Value) -> Result<jsonschema::Validator, ToolError> {
    if schema.to_string().len() > 65536 {
        return Err(ToolError::InvalidSchema("schema exceeds byte limit".into()));
    }
    check_schema(schema, 0, &mut 0)?;
    jsonschema::draft202012::options()
        .build(schema)
        .map_err(|error| ToolError::InvalidSchema(error.to_string()))
}

impl ToolRegistry {
    /// Register a runtime JSON-schema tool. Schemas compile once and arguments
    /// are validated before application dispatch. The initial subset has no refs,
    /// regex/format assertions, coercion, or implicit defaults.
    pub fn register_dynamic<F, Fut>(
        &mut self,
        definition: ToolDefinition,
        handler: F,
    ) -> Result<(), ToolError>
    where
        F: Fn(ToolInvocation, Value) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<Value, ToolError>> + Send + 'static,
    {
        let name = &definition.reference.name;
        if name.is_empty()
            || name.len() > 128
            || !name
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-' | b'.'))
            || definition.reference.revision.trim().is_empty()
            || definition.description.trim().is_empty()
            || definition.input_schema["type"] != "object"
            || definition.input_schema.to_string().len() > 65536
        {
            return Err(ToolError::InvalidDefinition);
        }
        if self.entries.contains_key(name) {
            return Err(ToolError::DuplicateDefinition);
        }
        let validator = Arc::new(compile_schema(&definition.input_schema)?);
        let handler = Arc::new(handler);
        let entry = Entry {
            definition,
            handler: Arc::new(move |context, input| {
                let validator = validator.clone();
                let handler = handler.clone();
                Box::pin(async move {
                    struct Limited(usize);
                    impl std::io::Write for Limited {
                        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                            self.0 = self
                                .0
                                .checked_sub(bytes.len())
                                .ok_or_else(|| std::io::Error::other("argument byte limit"))?;
                            Ok(bytes.len())
                        }
                        fn flush(&mut self) -> std::io::Result<()> {
                            Ok(())
                        }
                    }
                    serde_json::to_writer(Limited(65536), &input).map_err(|_| {
                        ToolError::InvalidArguments("arguments exceed byte limit".into())
                    })?;
                    if !validator.is_valid(&input) {
                        return Err(ToolError::InvalidArguments(
                            "arguments do not match the registered schema".into(),
                        ));
                    }
                    handler(context, input).await
                })
            }),
        };
        self.entries
            .insert(entry.definition.reference.name.clone(), entry);
        Ok(())
    }
}
