//! MCP adapter for a registry bound by the host to one participant/session/slot.
use crate::ActorId;
use crate::tools::{
    CancellationToken, ToolError, ToolGrant, ToolInvocation, ToolRegistry, ToolScope,
};
use rmcp::{ErrorData, RoleServer, ServerHandler, ServiceExt, model::*, service::RequestContext};
use std::sync::Arc;

pub struct McpToolServer {
    registry: Arc<ToolRegistry>,
    grant: ToolGrant,
    scope: ToolScope,
    actor: ActorId,
    host: ActorId,
    catalog: Vec<Tool>,
}
impl McpToolServer {
    /// The caller binds trusted scope and grants. MCP arguments never select them.
    pub fn new(
        registry: Arc<ToolRegistry>,
        grant: ToolGrant,
        scope: ToolScope,
        actor: ActorId,
        host: ActorId,
    ) -> Result<Self, ToolError> {
        let invocation = ToolInvocation {
            host: host.clone(),
            actor: actor.clone(),
            scope: scope.clone(),
            cancellation: CancellationToken::new(),
        };
        let catalog = registry
            .catalog(&grant, &invocation)?
            .into_iter()
            .map(|definition| {
                Tool::new(
                    definition.reference.name,
                    definition.description,
                    definition.input_schema.as_object().unwrap().clone(),
                )
            })
            .collect();
        Ok(Self {
            registry,
            grant,
            scope,
            actor,
            host,
            catalog,
        })
    }

    /// Own stdin/stdout until disconnect. Handlers must reserve stdout for MCP.
    pub async fn serve_stdio(self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.serve(rmcp::transport::stdio())
            .await?
            .waiting()
            .await?;
        Ok(())
    }
}

impl ServerHandler for McpToolServer {
    fn get_info(&self) -> ServerInfo {
        let mut info = ServerInfo::default();
        info.capabilities = ServerCapabilities::builder().enable_tools().build();
        info
    }
    async fn list_tools(
        &self,
        request: Option<PaginatedRequestParams>,
        _: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        if request.is_some_and(|request| request.cursor.is_some()) {
            return Err(ErrorData::invalid_params("unsupported tool cursor", None));
        }
        Ok(ListToolsResult {
            tools: self.catalog.clone(),
            ..Default::default()
        })
    }
    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        if request.task.is_some() {
            return Err(ErrorData::invalid_params(
                "MCP task execution is not supported",
                None,
            ));
        }
        let invocation = ToolInvocation {
            host: self.host.clone(),
            actor: self.actor.clone(),
            scope: self.scope.clone(),
            cancellation: context.ct,
        };
        let result = self
            .registry
            .invoke(
                &request.name,
                serde_json::Value::Object(request.arguments.unwrap_or_default()),
                &self.grant,
                invocation,
            )
            .await;
        Ok(match result {
            Ok(value) => CallToolResult::success(vec![ContentBlock::text(value.to_string())]),
            Err(error) => CallToolResult::error(vec![ContentBlock::text(error.to_string())]),
        })
    }
}
