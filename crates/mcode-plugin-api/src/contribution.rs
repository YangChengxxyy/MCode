//! Typed descriptors contributed through a plugin manifest.

// Rust guideline compliant 2026-08-26.

use std::collections::BTreeSet;
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::capability::{CapabilityDeclaration, CapabilityKind};
use crate::events::EventKind;
use crate::ids::Identifier;
use crate::limits::{
    MAX_CONTRIBUTIONS, MAX_DESCRIPTOR_JSON_BYTES, MAX_DESCRIPTORS_PER_KIND,
    MAX_PROMPT_CONTRIBUTION_BYTES,
};
use crate::path::resolve_contained_path;
use crate::ui::{TextTone, UiView, ViewContent, ViewMetadata};
use crate::validation::{valid_public_text, validate_json_value};

const MAX_MAILBOX_CAPACITY: u16 = 1024;

/// Metadata for a contributed tool.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ToolDescriptor {
    /// Stable contribution id.
    pub id: Identifier,
    /// Public callable tool name.
    pub name: Identifier,
    /// User-facing label.
    pub display_name: String,
    /// Model-facing description.
    pub description: String,
    /// Strict input JSON schema.
    pub input_schema: Value,
    /// Optional output JSON schema.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<Value>,
    /// Exact coarse capabilities needed by the adapter.
    #[serde(default)]
    pub required_capabilities: Vec<CapabilityKind>,
}

/// Metadata for a slash or host command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CommandDescriptor {
    /// Stable contribution id.
    pub id: Identifier,
    /// Public command name without a slash.
    pub name: Identifier,
    /// User-facing title.
    pub title: String,
    /// User-facing description.
    pub description: String,
}

/// Declaration for one bounded prompt contribution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PromptDescriptor {
    /// Stable contribution id.
    pub id: Identifier,
    /// Deterministic ordering priority.
    pub priority: i16,
    /// Maximum UTF-8 bytes returned for this contribution.
    pub max_bytes: usize,
}

/// Static resource kind understood by a future resource adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ResourceKind {
    /// Skill or instruction resource.
    Skill,
    /// Prompt template resource.
    Prompt,
    /// Theme resource.
    Theme,
    /// Agent/persona resource.
    Agent,
    /// Generic read-only data resource.
    Data,
}

/// Metadata for one plugin-root-contained resource.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResourceDescriptor {
    /// Stable resource id.
    pub id: Identifier,
    /// Resource kind.
    pub kind: ResourceKind,
    /// Portable path relative to the plugin root.
    pub path: String,
}

/// Declaration for one plugin-owned declarative view.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ViewDescriptor {
    /// Stable placement and invalidation metadata.
    pub metadata: ViewMetadata,
    /// User-facing purpose.
    pub description: String,
}

/// Declaration for one timeline item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TimelineDescriptor {
    /// Stable placement and invalidation metadata.
    pub metadata: ViewMetadata,
    /// User-facing purpose.
    pub description: String,
}

/// Declaration for one modal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModalDescriptor {
    /// Stable placement and invalidation metadata.
    pub metadata: ViewMetadata,
    /// User-facing purpose.
    pub description: String,
}

/// Declaration for one plugin-owned declarative widget.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WidgetDescriptor {
    /// Stable placement and invalidation metadata.
    pub metadata: ViewMetadata,
    /// User-facing purpose.
    pub description: String,
}

/// Declaration for one bounded plugin event mailbox subscription.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EventSubscriptionDescriptor {
    /// Stable subscription id.
    pub id: Identifier,
    /// Typed redacted event categories.
    pub events: Vec<EventKind>,
    /// Requested bounded mailbox capacity; host policy may lower it.
    pub mailbox_capacity: u16,
}

/// All typed contributions declared by one plugin.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Contributions {
    /// Tool descriptors.
    #[serde(default)]
    pub tools: Vec<ToolDescriptor>,
    /// Command descriptors.
    #[serde(default)]
    pub commands: Vec<CommandDescriptor>,
    /// Prompt descriptors.
    #[serde(default)]
    pub prompts: Vec<PromptDescriptor>,
    /// Static resource descriptors.
    #[serde(default)]
    pub resources: Vec<ResourceDescriptor>,
    /// Declarative view descriptors.
    #[serde(default)]
    pub views: Vec<ViewDescriptor>,
    /// Timeline descriptors.
    #[serde(default)]
    pub timelines: Vec<TimelineDescriptor>,
    /// Modal descriptors.
    #[serde(default)]
    pub modals: Vec<ModalDescriptor>,
    /// Widget descriptors.
    #[serde(default)]
    pub widgets: Vec<WidgetDescriptor>,
    /// Redacted event subscriptions.
    #[serde(default)]
    pub event_subscriptions: Vec<EventSubscriptionDescriptor>,
}

impl Contributions {
    /// Returns the total number of descriptors.
    #[must_use]
    pub fn len(&self) -> usize {
        self.tools.len()
            + self.commands.len()
            + self.prompts.len()
            + self.resources.len()
            + self.views.len()
            + self.timelines.len()
            + self.modals.len()
            + self.widgets.len()
            + self.event_subscriptions.len()
    }

    /// Returns whether no descriptors are declared.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Validates descriptor bounds and capability relationships.
    ///
    /// # Errors
    ///
    /// Returns [`ContributionValidationError`] for malformed schemas, paths,
    /// duplicate ids/names, missing capabilities, or any descriptor limit.
    pub fn validate(
        &self,
        plugin_root: &Path,
        capabilities: &[CapabilityDeclaration],
    ) -> Result<(), ContributionValidationError> {
        if self.len() > MAX_CONTRIBUTIONS
            || [
                self.tools.len(),
                self.commands.len(),
                self.prompts.len(),
                self.resources.len(),
                self.views.len(),
                self.timelines.len(),
                self.modals.len(),
                self.widgets.len(),
                self.event_subscriptions.len(),
            ]
            .into_iter()
            .any(|count| count > MAX_DESCRIPTORS_PER_KIND)
        {
            return Err(ContributionValidationError::TooMany);
        }
        validate_unique_ids(self)?;
        let declared_kinds: BTreeSet<_> = capabilities
            .iter()
            .map(CapabilityDeclaration::kind)
            .collect();

        for tool in &self.tools {
            validate_text_fields(&[&tool.display_name, &tool.description])?;
            validate_schema(&tool.input_schema)?;
            if let Some(schema) = &tool.output_schema {
                validate_schema(schema)?;
            }
            let required: BTreeSet<_> = tool.required_capabilities.iter().copied().collect();
            if required.len() != tool.required_capabilities.len()
                || !required.is_subset(&declared_kinds)
            {
                return Err(ContributionValidationError::MissingCapability);
            }
        }
        for command in &self.commands {
            validate_text_fields(&[&command.title, &command.description])?;
        }
        if !self.prompts.is_empty() && !declared_kinds.contains(&CapabilityKind::PromptContribution)
        {
            return Err(ContributionValidationError::MissingCapability);
        }
        for prompt in &self.prompts {
            if prompt.max_bytes == 0 || prompt.max_bytes > MAX_PROMPT_CONTRIBUTION_BYTES {
                return Err(ContributionValidationError::InvalidLimit);
            }
        }
        for resource in &self.resources {
            resolve_contained_path(plugin_root, &resource.path)
                .map_err(|_| ContributionValidationError::UnsafePath)?;
        }
        let ui_count =
            self.views.len() + self.timelines.len() + self.modals.len() + self.widgets.len();
        if ui_count > 0 && !declared_kinds.contains(&CapabilityKind::Ui) {
            return Err(ContributionValidationError::MissingCapability);
        }
        for (description, metadata, probe) in ui_probes(self) {
            validate_text_fields(&[description])?;
            probe
                .validate()
                .map_err(|_| ContributionValidationError::InvalidWidget)?;
            let _ = metadata;
        }
        for subscription in &self.event_subscriptions {
            let unique: BTreeSet<_> = subscription.events.iter().copied().collect();
            if subscription.events.is_empty()
                || unique.len() != subscription.events.len()
                || subscription.mailbox_capacity == 0
                || subscription.mailbox_capacity > MAX_MAILBOX_CAPACITY
            {
                return Err(ContributionValidationError::InvalidSubscription);
            }
        }
        Ok(())
    }

    /// Returns whether at least one subscription includes `kind`.
    #[must_use]
    pub fn subscribes_to(&self, kind: EventKind) -> bool {
        self.event_subscriptions
            .iter()
            .any(|subscription| subscription.events.contains(&kind))
    }

    /// Returns the lowest requested mailbox capacity, if any.
    #[must_use]
    pub fn requested_mailbox_capacity(&self) -> Option<usize> {
        self.event_subscriptions
            .iter()
            .map(|subscription| usize::from(subscription.mailbox_capacity))
            .min()
    }
}

fn ui_probes(contributions: &Contributions) -> Vec<(&str, &ViewMetadata, UiView)> {
    let sample = ViewContent::Text {
        text: "descriptor validation".into(),
        tone: TextTone::Normal,
        emphasized: false,
    };
    let mut probes = Vec::new();
    for view in &contributions.views {
        probes.push((
            view.description.as_str(),
            &view.metadata,
            UiView::Panel {
                metadata: view.metadata.clone(),
                content: sample.clone(),
            },
        ));
    }
    for timeline in &contributions.timelines {
        probes.push((
            timeline.description.as_str(),
            &timeline.metadata,
            UiView::Timeline {
                metadata: timeline.metadata.clone(),
                content: sample.clone(),
            },
        ));
    }
    for modal in &contributions.modals {
        probes.push((
            modal.description.as_str(),
            &modal.metadata,
            UiView::Modal {
                metadata: modal.metadata.clone(),
                content: sample.clone(),
            },
        ));
    }
    for widget in &contributions.widgets {
        probes.push((
            widget.description.as_str(),
            &widget.metadata,
            UiView::Widget {
                metadata: widget.metadata.clone(),
                content: sample.clone(),
            },
        ));
    }
    probes
}

fn validate_unique_ids(contributions: &Contributions) -> Result<(), ContributionValidationError> {
    let mut ids = BTreeSet::new();
    for id in contributions
        .tools
        .iter()
        .map(|item| &item.id)
        .chain(contributions.commands.iter().map(|item| &item.id))
        .chain(contributions.prompts.iter().map(|item| &item.id))
        .chain(contributions.resources.iter().map(|item| &item.id))
        .chain(contributions.views.iter().map(|item| &item.metadata.id))
        .chain(contributions.timelines.iter().map(|item| &item.metadata.id))
        .chain(contributions.modals.iter().map(|item| &item.metadata.id))
        .chain(contributions.widgets.iter().map(|item| &item.metadata.id))
        .chain(
            contributions
                .event_subscriptions
                .iter()
                .map(|item| &item.id),
        )
    {
        if !ids.insert(id) {
            return Err(ContributionValidationError::Duplicate);
        }
    }
    ensure_unique(contributions.tools.iter().map(|item| &item.name))?;
    ensure_unique(contributions.commands.iter().map(|item| &item.name))?;
    Ok(())
}

fn ensure_unique<'a>(
    values: impl Iterator<Item = &'a Identifier>,
) -> Result<(), ContributionValidationError> {
    let mut unique = BTreeSet::new();
    for value in values {
        if !unique.insert(value) {
            return Err(ContributionValidationError::Duplicate);
        }
    }
    Ok(())
}

fn validate_schema(value: &Value) -> Result<(), ContributionValidationError> {
    if !value.is_object() {
        return Err(ContributionValidationError::InvalidSchema);
    }
    validate_json_value(value, MAX_DESCRIPTOR_JSON_BYTES)
        .map_err(|_| ContributionValidationError::InvalidSchema)?;
    Ok(())
}

fn validate_text_fields(values: &[&str]) -> Result<(), ContributionValidationError> {
    if values.iter().any(|value| !valid_public_text(value, 4096)) {
        return Err(ContributionValidationError::InvalidText);
    }
    Ok(())
}

/// Invalid contribution descriptor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ContributionValidationError {
    /// Too many descriptors were declared.
    #[error("plugin declares too many contributions")]
    TooMany,
    /// A stable id or callable name was duplicated.
    #[error("plugin contribution identifier is duplicated")]
    Duplicate,
    /// Human/model-facing text was empty, oversized, or contained controls.
    #[error("plugin contribution text is invalid")]
    InvalidText,
    /// A JSON schema was missing, malformed, oversized, or too deep.
    #[error("plugin contribution JSON schema is invalid")]
    InvalidSchema,
    /// A resource path escaped the plugin root.
    #[error("plugin resource path is unsafe")]
    UnsafePath,
    /// A descriptor required a capability not declared by the plugin.
    #[error("plugin contribution is missing a required capability declaration")]
    MissingCapability,
    /// A descriptor byte or capacity limit was invalid.
    #[error("plugin contribution limit is invalid")]
    InvalidLimit,
    /// Widget or view layout metadata was invalid.
    #[error("plugin widget descriptor is invalid")]
    InvalidWidget,
    /// Event subscription was empty, duplicated, or unbounded.
    #[error("plugin event subscription is invalid")]
    InvalidSubscription,
}
