//! Immutable, transactional MCP catalog snapshots.

// Rust guideline compliant 2026-08-20.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    config::OutputLimits,
    error::{Error, ErrorKind, Recovery, Result},
    identity::{ItemKind, NamespacedId, ServerName},
    protocol::{RemotePrompt, RemoteResource, RemoteResourceTemplate, RemoteTool},
    validation::{
        sanitize_json, validate_catalog_text, validate_json_schema, validate_tool_schema,
    },
};

/// A monotonically increasing catalog generation.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Generation(u64);

impl Generation {
    /// Creates a generation from its persisted-free counter value.
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Returns the counter value.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }

    /// Returns the next generation, saturating only at `u64::MAX`.
    #[must_use]
    pub const fn next(self) -> Self {
        Self(self.0.saturating_add(1))
    }
}

/// A catalog section refreshed by a list-change notification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum CatalogSection {
    /// Tools.
    Tools,
    /// Resources and resource templates as one transaction.
    Resources,
    /// Prompts.
    Prompts,
}

/// Validated tool metadata with stable provenance.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogTool {
    /// Stable `mcp:<server>:<item>` identity.
    pub id: NamespacedId,
    /// Server-provided name used on the wire.
    pub remote_name: String,
    /// Optional title.
    pub title: Option<String>,
    /// Optional description.
    pub description: Option<String>,
    /// Bounded input JSON Schema.
    pub input_schema: Value,
    /// Optional bounded output JSON Schema.
    pub output_schema: Option<Value>,
    /// Untrusted display hints; never permission authority.
    pub annotations: Option<Value>,
}

/// Validated concrete resource metadata with stable provenance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogResource {
    /// Stable identity.
    pub id: NamespacedId,
    /// Resource URI.
    pub uri: String,
    /// Server-provided name.
    pub remote_name: String,
    /// Optional title.
    pub title: Option<String>,
    /// Optional description.
    pub description: Option<String>,
    /// Optional MIME type.
    pub mime_type: Option<String>,
    /// Optional raw byte size.
    pub size: Option<u64>,
}

/// Validated resource-template metadata with stable provenance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogResourceTemplate {
    /// Stable identity.
    pub id: NamespacedId,
    /// RFC 6570 URI template.
    pub uri_template: String,
    /// Server-provided name.
    pub remote_name: String,
    /// Optional title.
    pub title: Option<String>,
    /// Optional description.
    pub description: Option<String>,
    /// Optional MIME type.
    pub mime_type: Option<String>,
}

/// Validated prompt argument metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogPromptArgument {
    /// Argument name.
    pub name: String,
    /// Optional title.
    pub title: Option<String>,
    /// Optional description.
    pub description: Option<String>,
    /// Whether the argument is required.
    pub required: bool,
}

/// Validated prompt metadata with stable provenance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogPrompt {
    /// Stable identity.
    pub id: NamespacedId,
    /// Server-provided name used on the wire.
    pub remote_name: String,
    /// Optional title.
    pub title: Option<String>,
    /// Optional description.
    pub description: Option<String>,
    /// Validated argument declarations.
    pub arguments: Vec<CatalogPromptArgument>,
}

/// Complete remote catalog material before transactional validation.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CatalogParts {
    /// Fully paginated tools.
    pub tools: Vec<RemoteTool>,
    /// Fully paginated resources.
    pub resources: Vec<RemoteResource>,
    /// Fully paginated resource templates.
    pub resource_templates: Vec<RemoteResourceTemplate>,
    /// Fully paginated prompts.
    pub prompts: Vec<RemotePrompt>,
}

/// One immutable per-server catalog generation.
#[derive(Debug, Clone, PartialEq)]
pub struct CatalogSnapshot {
    server: ServerName,
    generation: Generation,
    tools: BTreeMap<NamespacedId, CatalogTool>,
    resources: BTreeMap<NamespacedId, CatalogResource>,
    resource_templates: BTreeMap<NamespacedId, CatalogResourceTemplate>,
    prompts: BTreeMap<NamespacedId, CatalogPrompt>,
}

impl CatalogSnapshot {
    /// Creates an empty initial snapshot.
    #[must_use]
    pub fn empty(server: ServerName) -> Self {
        Self {
            server,
            generation: Generation::default(),
            tools: BTreeMap::new(),
            resources: BTreeMap::new(),
            resource_templates: BTreeMap::new(),
            prompts: BTreeMap::new(),
        }
    }

    /// Validates every section and commits one immutable snapshot.
    ///
    /// No partial catalog escapes if any item, schema, or identity is invalid.
    ///
    /// # Errors
    ///
    /// Returns a validation or explicit collision error.
    pub fn build(
        server: ServerName,
        generation: Generation,
        parts: CatalogParts,
        limits: &OutputLimits,
    ) -> Result<Self> {
        enforce_section_caps(&server, &parts, limits)?;
        let mut identities = BTreeMap::new();
        let tools = build_tools(&server, parts.tools, limits, &mut identities)?;
        let resources = build_resources(&server, parts.resources, limits, &mut identities)?;
        let resource_templates =
            build_templates(&server, parts.resource_templates, limits, &mut identities)?;
        let prompts = build_prompts(&server, parts.prompts, limits, &mut identities)?;
        Ok(Self {
            server,
            generation,
            tools,
            resources,
            resource_templates,
            prompts,
        })
    }

    /// Returns the source server.
    #[must_use]
    pub fn server(&self) -> &ServerName {
        &self.server
    }

    /// Returns this immutable generation.
    #[must_use]
    pub const fn generation(&self) -> Generation {
        self.generation
    }

    /// Returns tools sorted by stable identity.
    pub fn tools(&self) -> impl ExactSizeIterator<Item = &CatalogTool> {
        self.tools.values()
    }

    /// Returns resources sorted by stable identity.
    pub fn resources(&self) -> impl ExactSizeIterator<Item = &CatalogResource> {
        self.resources.values()
    }

    /// Returns resource templates sorted by stable identity.
    pub fn resource_templates(&self) -> impl ExactSizeIterator<Item = &CatalogResourceTemplate> {
        self.resource_templates.values()
    }

    /// Returns prompts sorted by stable identity.
    pub fn prompts(&self) -> impl ExactSizeIterator<Item = &CatalogPrompt> {
        self.prompts.values()
    }

    /// Looks up a tool by stable identity.
    #[must_use]
    pub fn tool(&self, id: &NamespacedId) -> Option<&CatalogTool> {
        self.tools.get(id)
    }

    /// Looks up a prompt by stable identity.
    #[must_use]
    pub fn prompt(&self, id: &NamespacedId) -> Option<&CatalogPrompt> {
        self.prompts.get(id)
    }

    /// Returns every stable identity, including cross-section provenance.
    #[must_use]
    pub fn identities(&self) -> BTreeSet<NamespacedId> {
        self.tools
            .keys()
            .chain(self.resources.keys())
            .chain(self.resource_templates.keys())
            .chain(self.prompts.keys())
            .cloned()
            .collect()
    }

    /// Copies this snapshot into refreshable remote parts.
    #[must_use]
    pub fn to_parts(&self) -> CatalogParts {
        CatalogParts {
            tools: self
                .tools
                .values()
                .map(|tool| RemoteTool {
                    name: tool.remote_name.clone(),
                    title: tool.title.clone(),
                    description: tool.description.clone(),
                    input_schema: tool.input_schema.clone(),
                    output_schema: tool.output_schema.clone(),
                    annotations: tool.annotations.clone(),
                })
                .collect(),
            resources: self
                .resources
                .values()
                .map(|resource| RemoteResource {
                    uri: resource.uri.clone(),
                    name: resource.remote_name.clone(),
                    title: resource.title.clone(),
                    description: resource.description.clone(),
                    mime_type: resource.mime_type.clone(),
                    size: resource.size,
                })
                .collect(),
            resource_templates: self
                .resource_templates
                .values()
                .map(|template| RemoteResourceTemplate {
                    uri_template: template.uri_template.clone(),
                    name: template.remote_name.clone(),
                    title: template.title.clone(),
                    description: template.description.clone(),
                    mime_type: template.mime_type.clone(),
                })
                .collect(),
            prompts: self
                .prompts
                .values()
                .map(|prompt| RemotePrompt {
                    name: prompt.remote_name.clone(),
                    title: prompt.title.clone(),
                    description: prompt.description.clone(),
                    arguments: prompt
                        .arguments
                        .iter()
                        .map(|argument| crate::protocol::RemotePromptArgument {
                            name: argument.name.clone(),
                            title: argument.title.clone(),
                            description: argument.description.clone(),
                            required: argument.required,
                        })
                        .collect(),
                })
                .collect(),
        }
    }
}

fn build_tools(
    server: &ServerName,
    tools: Vec<RemoteTool>,
    limits: &OutputLimits,
    identities: &mut BTreeMap<NamespacedId, ItemKind>,
) -> Result<BTreeMap<NamespacedId, CatalogTool>> {
    let mut output = BTreeMap::new();
    for tool in tools {
        let id = claim_identity(server, &tool.name, ItemKind::Tool, identities)?;
        let input_schema = validate_tool_schema(server, &tool.input_schema, limits)?;
        let output_schema = tool
            .output_schema
            .as_ref()
            .map(|schema| validate_output_schema(server, schema, limits))
            .transpose()?;
        let annotations = tool
            .annotations
            .as_ref()
            .map(|value| sanitize_json(server, value, limits))
            .transpose()?;
        let item = CatalogTool {
            id: id.clone(),
            remote_name: tool.name,
            title: optional_text(server, tool.title, limits)?,
            description: optional_text(server, tool.description, limits)?,
            input_schema,
            output_schema,
            annotations,
        };
        output.insert(id, item);
    }
    Ok(output)
}

fn build_resources(
    server: &ServerName,
    resources: Vec<RemoteResource>,
    limits: &OutputLimits,
    identities: &mut BTreeMap<NamespacedId, ItemKind>,
) -> Result<BTreeMap<NamespacedId, CatalogResource>> {
    let mut output = BTreeMap::new();
    for resource in resources {
        let id = claim_identity(server, &resource.name, ItemKind::Resource, identities)?;
        let item = CatalogResource {
            id: id.clone(),
            uri: required_text(server, resource.uri, limits, "resource URI")?,
            remote_name: resource.name,
            title: optional_text(server, resource.title, limits)?,
            description: optional_text(server, resource.description, limits)?,
            mime_type: optional_text(server, resource.mime_type, limits)?,
            size: resource.size,
        };
        output.insert(id, item);
    }
    Ok(output)
}

fn build_templates(
    server: &ServerName,
    templates: Vec<RemoteResourceTemplate>,
    limits: &OutputLimits,
    identities: &mut BTreeMap<NamespacedId, ItemKind>,
) -> Result<BTreeMap<NamespacedId, CatalogResourceTemplate>> {
    let mut output = BTreeMap::new();
    for template in templates {
        let id = claim_identity(
            server,
            &template.name,
            ItemKind::ResourceTemplate,
            identities,
        )?;
        let item = CatalogResourceTemplate {
            id: id.clone(),
            uri_template: required_text(
                server,
                template.uri_template,
                limits,
                "resource URI template",
            )?,
            remote_name: template.name,
            title: optional_text(server, template.title, limits)?,
            description: optional_text(server, template.description, limits)?,
            mime_type: optional_text(server, template.mime_type, limits)?,
        };
        output.insert(id, item);
    }
    Ok(output)
}

fn build_prompts(
    server: &ServerName,
    prompts: Vec<RemotePrompt>,
    limits: &OutputLimits,
    identities: &mut BTreeMap<NamespacedId, ItemKind>,
) -> Result<BTreeMap<NamespacedId, CatalogPrompt>> {
    let mut output = BTreeMap::new();
    for prompt in prompts {
        let id = claim_identity(server, &prompt.name, ItemKind::Prompt, identities)?;
        if prompt.arguments.len() > 256 {
            return Err(validation_error(server, "prompt has too many arguments"));
        }
        let mut names = BTreeSet::new();
        let mut arguments = Vec::with_capacity(prompt.arguments.len());
        for argument in prompt.arguments {
            let name = required_text(server, argument.name, limits, "prompt argument name")?;
            if !names.insert(name.clone()) {
                return Err(conflict_error(server, "duplicate prompt argument name"));
            }
            arguments.push(CatalogPromptArgument {
                name,
                title: optional_text(server, argument.title, limits)?,
                description: optional_text(server, argument.description, limits)?,
                required: argument.required,
            });
        }
        let item = CatalogPrompt {
            id: id.clone(),
            remote_name: prompt.name,
            title: optional_text(server, prompt.title, limits)?,
            description: optional_text(server, prompt.description, limits)?,
            arguments,
        };
        output.insert(id, item);
    }
    Ok(output)
}

fn claim_identity(
    server: &ServerName,
    item: &str,
    kind: ItemKind,
    identities: &mut BTreeMap<NamespacedId, ItemKind>,
) -> Result<NamespacedId> {
    let id = NamespacedId::new(server.clone(), item)?;
    if let Some(previous) = identities.insert(id.clone(), kind) {
        return Err(conflict_error(
            server,
            format!("catalog identity {id} collides between {previous:?} and {kind:?}"),
        ));
    }
    Ok(id)
}

fn validate_output_schema(
    server: &ServerName,
    schema: &Value,
    limits: &OutputLimits,
) -> Result<Value> {
    validate_json_schema(server, schema, limits)
}

fn enforce_section_caps(
    server: &ServerName,
    parts: &CatalogParts,
    limits: &OutputLimits,
) -> Result<()> {
    if [
        parts.tools.len(),
        parts.resources.len(),
        parts.resource_templates.len(),
        parts.prompts.len(),
    ]
    .iter()
    .any(|size| *size > limits.max_catalog_items)
    {
        return Err(validation_error(server, "catalog section exceeds item cap"));
    }
    Ok(())
}

fn optional_text(
    server: &ServerName,
    value: Option<String>,
    limits: &OutputLimits,
) -> Result<Option<String>> {
    value
        .map(|value| validate_catalog_text(server, &value, limits))
        .transpose()
}

fn required_text(
    server: &ServerName,
    value: String,
    limits: &OutputLimits,
    label: &str,
) -> Result<String> {
    if value.is_empty() {
        return Err(validation_error(server, format!("{label} is empty")));
    }
    validate_catalog_text(server, &value, limits)
}

fn validation_error(server: &ServerName, message: impl AsRef<str>) -> Error {
    Error::new(ErrorKind::Validation, Recovery::Fatal, message).with_server(server.clone())
}

fn conflict_error(server: &ServerName, message: impl AsRef<str>) -> Error {
    Error::new(ErrorKind::Conflict, Recovery::Fatal, message).with_server(server.clone())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn tool(name: &str) -> RemoteTool {
        RemoteTool {
            name: name.to_owned(),
            title: None,
            description: None,
            input_schema: json!({"type":"object"}),
            output_schema: None,
            annotations: None,
        }
    }

    #[test]
    fn cross_section_collision_is_explicit() {
        let server = ServerName::new("s").unwrap();
        let parts = CatalogParts {
            tools: vec![tool("same")],
            prompts: vec![RemotePrompt {
                name: "same".into(),
                title: None,
                description: None,
                arguments: vec![],
            }],
            ..CatalogParts::default()
        };
        let error =
            CatalogSnapshot::build(server, Generation::new(1), parts, &OutputLimits::default())
                .unwrap_err();
        assert_eq!(error.kind(), ErrorKind::Conflict);
    }

    #[test]
    fn snapshot_is_sorted_and_provenanced() {
        let server = ServerName::new("context7").unwrap();
        let snapshot = CatalogSnapshot::build(
            server,
            Generation::new(3),
            CatalogParts {
                tools: vec![tool("z"), tool("a")],
                ..CatalogParts::default()
            },
            &OutputLimits::default(),
        )
        .unwrap();
        let ids: Vec<_> = snapshot.tools().map(|tool| tool.id.to_string()).collect();
        assert_eq!(ids, ["mcp:context7:a", "mcp:context7:z"]);
        assert_eq!(snapshot.generation().get(), 3);
    }
}
