//! Message model — the core vocabulary exchanged between user, model, tools,
//! and plugins (design doc `01-agent-core.md` §1).
//!
//! Serde uses the default externally-tagged representation here. Provider
//! wire handling remains in `mcode-llm`; durable Session encoding belongs to
//! the future signed Session Pack behind `SessionPackService`, not Core.

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use url::Url;

/// Phase of one assistant text segment within a response.
///
/// Providers that split a single response into several assistant
/// messages (OpenAI Responses `phase`) mark each segment as
/// intermediate commentary or the final answer. The phase must be
/// preserved verbatim when the segment is replayed to the same wire
/// protocol; other protocols ignore it, and user or tool text never
/// carries a phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssistantPhase {
    /// Intermediate user-visible commentary, such as a preamble
    /// before tool calls.
    Commentary,
    /// The completed final answer of the response.
    FinalAnswer,
}

impl AssistantPhase {
    /// Returns the stable serialized name (`"commentary"` or
    /// `"final_answer"`); identical to the serde representation.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Commentary => "commentary",
            Self::FinalAnswer => "final_answer",
        }
    }

    /// Parses a provider phase name; unknown names yield `None`.
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "commentary" => Some(Self::Commentary),
            "final_answer" => Some(Self::FinalAnswer),
            _ => None,
        }
    }
}

/// A message in the conversation tree.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Message {
    /// A message authored by the user (prompts, steers, follow-ups).
    User(UserMessage),
    /// A message produced by the model.
    Assistant(AssistantMessage),
    /// The result of executing a tool call.
    ToolResult(ToolResultMessage),
    /// Plugin-defined message. The `data` payload passes through
    /// serialization untouched so plugins can persist arbitrary state —
    /// the Rust replacement for pi's declaration merging.
    Custom(CustomMessage),
}

/// A user-authored message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UserMessage {
    pub content: Vec<ContentBlock>,
}

impl UserMessage {
    /// Build a plain-text user message.
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            content: vec![ContentBlock::Text(TextBlock::new(text))],
        }
    }
}

/// A model-produced message: ordered content blocks plus turn metadata.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AssistantMessage {
    pub blocks: Vec<ContentBlock>,
    /// Token usage as reported by the provider, when available.
    pub usage: Option<Usage>,
    pub stop_reason: StopReason,
}

/// A single unit of message content.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ContentBlock {
    /// Plain text (with optional assistant phase metadata).
    Text(TextBlock),
    /// Model reasoning ("thinking") content and opaque replay state.
    Thinking(ThinkingBlock),
    /// A request to invoke a tool.
    ToolCall(ToolCall),
    /// Binary payload (currently only used for images).
    Image(BinaryData),
}

/// Text content plus assistant phase provenance.
///
/// Phase-less text serializes as a plain string; phased assistant
/// text uses `{ "text": …, "phase": … }` to keep payloads compact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextBlock {
    /// The text itself.
    pub text: String,
    /// Assistant-message phase, when the producing provider
    /// distinguishes commentary from the final answer.
    pub phase: Option<AssistantPhase>,
}

impl TextBlock {
    /// Creates phase-less text.
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            phase: None,
        }
    }

    /// Attaches an assistant phase.
    pub fn with_phase(mut self, phase: AssistantPhase) -> Self {
        self.phase = Some(phase);
        self
    }
}

impl From<String> for TextBlock {
    fn from(text: String) -> Self {
        Self::new(text)
    }
}

impl From<&str> for TextBlock {
    fn from(text: &str) -> Self {
        Self::new(text)
    }
}

impl Serialize for TextBlock {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        if let Some(phase) = self.phase {
            #[derive(Serialize)]
            struct PhasedText<'a> {
                text: &'a str,
                phase: AssistantPhase,
            }
            return PhasedText {
                text: &self.text,
                phase,
            }
            .serialize(serializer);
        }
        serializer.serialize_str(&self.text)
    }
}

impl<'de> Deserialize<'de> for TextBlock {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum TextRepresentation {
            Plain(String),
            Phased { text: String, phase: AssistantPhase },
        }
        match TextRepresentation::deserialize(deserializer)? {
            TextRepresentation::Plain(text) => Ok(Self::new(text)),
            TextRepresentation::Phased { text, phase } => Ok(Self {
                text,
                phase: Some(phase),
            }),
        }
    }
}

/// Wire protocol family that an opaque replay payload belongs to.
///
/// The family is a *necessary* condition for replay: opaque state is
/// only meaningful to the wire protocol that produced it. It is not a
/// *sufficient* one — see [`ReplayDomain`] for the trust rule that
/// decides verbatim replay. Any replay-incompatible combination
/// deterministically receives the portable visible content with the
/// wire-only state stripped, so foreign state can never construct an
/// invalid wire request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplayWire {
    /// Anthropic `POST /v1/messages`.
    AnthropicMessages,
    /// OpenAI `POST /responses`.
    OpenAiResponses,
    /// OpenAI-compatible `POST /chat/completions`.
    OpenAiChatCompletions,
}

/// Replay trust policy of the provider profile serving a conversation.
///
/// Opaque replay payloads are bound to the backend that produced them:
/// an Anthropic thinking signature or redacted payload and an OpenAI
/// encrypted reasoning item are only meaningful — and only private — to
/// the endpoint that created them. A profile can point the same wire
/// protocol at any host, and a profile id alone is not an endpoint
/// identity: built-in ids such as `openai` keep their id while a
/// base-URL override repoints them at an arbitrary host. Wire equality
/// and id equality are therefore both necessary but not sufficient
/// trust boundaries. Verbatim replay requires the consuming profile to
/// be the producer itself **on the same endpoint origin**, or the
/// producer to be explicitly trusted for gateway sharing. Every other
/// combination — different wire, untrusted same-wire profile, the same
/// id on a different endpoint, or state with unknown provenance —
/// receives the stripped portable downgrade.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplayDomain {
    /// Wire protocol spoken by the consuming profile.
    pub wire: ReplayWire,
    /// Id of the consuming provider profile.
    pub provider: String,
    /// Origin (`scheme://host[:port]`) of the consuming profile's
    /// effective endpoint, i.e. its base URL after any environment
    /// override. Self-produced state only replays verbatim when this
    /// equals the producer's recorded origin.
    pub endpoint: String,
    /// Producing profile ids this consumer explicitly trusts to share
    /// replay state, beyond itself (gateways known to share a backend).
    /// Explicit trust crosses profile ids and endpoint origins alike,
    /// but both origins must still be present and valid.
    pub trusted: Vec<String>,
}

impl ReplayDomain {
    /// Creates a domain that trusts only `provider` itself on
    /// `endpoint`.
    pub fn new(wire: ReplayWire, provider: impl Into<String>, endpoint: impl Into<String>) -> Self {
        Self {
            wire,
            provider: provider.into(),
            endpoint: endpoint.into(),
            trusted: Vec::new(),
        }
    }

    /// Explicitly trusts one additional producing profile id.
    pub fn with_trusted(mut self, provider: impl Into<String>) -> Self {
        self.trusted.push(provider.into());
        self
    }
}

/// Provider-owned replay state with explicit provenance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplayState {
    /// Wire protocol that produced — and may replay — this payload.
    pub wire: ReplayWire,
    /// Id of the provider profile that produced the payload, when
    /// known. Part of the replay trust boundary enforced by
    /// [`is_replayable_on`](Self::is_replayable_on): a payload only
    /// replays verbatim to its producer or to a profile that explicitly
    /// trusts it.
    pub provider: Option<String>,
    /// Origin (`scheme://host[:port]`) of the endpoint that produced the
    /// payload, when known. A profile id is not an endpoint identity —
    /// built-in ids survive base-URL overrides — so self-produced state
    /// additionally replays only on the same origin.
    pub endpoint: Option<String>,
    /// Opaque payload, replayed verbatim to the same wire protocol only.
    pub data: String,
    /// Whether `data` holds provider-redacted reasoning instead of a
    /// signed reasoning payload.
    pub redacted: bool,
}

impl ReplayState {
    /// Creates unsigned state for `wire` with unknown producer.
    pub fn new(wire: ReplayWire, data: impl Into<String>) -> Self {
        Self {
            wire,
            provider: None,
            endpoint: None,
            data: data.into(),
            redacted: false,
        }
    }

    /// Records the producing provider profile id.
    pub fn with_provider(mut self, provider: impl Into<String>) -> Self {
        self.provider = Some(provider.into());
        self
    }

    /// Records the origin of the endpoint that produced the payload.
    pub fn with_endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.endpoint = Some(endpoint.into());
        self
    }

    /// Marks the payload as provider-redacted reasoning.
    pub fn with_redacted(mut self, redacted: bool) -> Self {
        self.redacted = redacted;
        self
    }

    /// Returns whether this state may be replayed verbatim inside
    /// `domain`.
    ///
    /// The explicit compatibility rule: the consuming profile must speak
    /// the same wire protocol **and** either be the producer itself on
    /// the same endpoint origin, or explicitly trust the producer.
    /// Both producer and consumer must carry valid endpoint-origin
    /// provenance even under explicit trust. Incomplete or malformed
    /// provenance never replays verbatim; every incompatible combination
    /// receives the stripped downgrade instead.
    pub fn is_replayable_on(&self, domain: &ReplayDomain) -> bool {
        if self.wire != domain.wire {
            return false;
        }
        let Some(producer) = self.provider.as_deref() else {
            return false;
        };
        let Some(producer_endpoint) = self
            .endpoint
            .as_deref()
            .filter(|endpoint| is_endpoint_origin(endpoint))
        else {
            return false;
        };
        if !is_endpoint_origin(&domain.endpoint) {
            return false;
        }

        (producer == domain.provider.as_str() && producer_endpoint == domain.endpoint.as_str())
            || domain.trusted.iter().any(|trusted| trusted == producer)
    }
}

/// Returns whether `value` is a complete HTTP(S) endpoint origin.
///
/// Provider profiles emit canonical origins. This defensive check also
/// covers replay state loaded from serialized history or assembled by callers,
/// where missing provenance is represented by `None` or an empty value.
fn is_endpoint_origin(value: &str) -> bool {
    let Ok(url) = Url::parse(value) else {
        return false;
    };
    matches!(url.scheme(), "http" | "https")
        && url.host_str().is_some()
        && url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none()
        && url.origin().ascii_serialization() == value
}

/// Model reasoning plus provider-owned state required for later turns.
///
/// `replay` is opaque to `mcode-core` but carries explicit provenance:
/// the wire protocol (and, when known, the provider profile id) that
/// produced it. Anthropic stores its thinking signature or encrypted
/// redacted payload; OpenAI Responses stores the complete JSON
/// reasoning item, including its id and encrypted content. Replay
/// rules live on [`ReplayState`] and [`ReplayDomain`]. Blocks without
/// replay state use the compact string-only representation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThinkingBlock {
    /// Human-visible reasoning or summary text.
    pub text: String,
    /// Opaque provider state that must be replayed unchanged.
    pub replay: Option<ReplayState>,
}

impl ThinkingBlock {
    /// Creates reasoning without provider replay state.
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            replay: None,
        }
    }

    /// Attaches opaque provider replay state with provenance.
    pub fn with_replay(mut self, replay: ReplayState) -> Self {
        self.replay = Some(replay);
        self
    }
}

impl From<String> for ThinkingBlock {
    fn from(text: String) -> Self {
        Self::new(text)
    }
}

impl From<&str> for ThinkingBlock {
    fn from(text: &str) -> Self {
        Self::new(text)
    }
}

impl Serialize for ReplayState {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        #[derive(Serialize)]
        struct RichReplay<'a> {
            wire: ReplayWire,
            #[serde(skip_serializing_if = "Option::is_none")]
            provider: Option<&'a str>,
            #[serde(skip_serializing_if = "Option::is_none")]
            endpoint: Option<&'a str>,
            data: &'a str,
            #[serde(default, skip_serializing_if = "std::ops::Not::not")]
            redacted: bool,
        }

        RichReplay {
            wire: self.wire,
            provider: self.provider.as_deref(),
            endpoint: self.endpoint.as_deref(),
            data: &self.data,
            redacted: self.redacted,
        }
        .serialize(serializer)
    }
}

impl Serialize for ThinkingBlock {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let Some(replay) = &self.replay else {
            return serializer.serialize_str(&self.text);
        };

        #[derive(Serialize)]
        struct RichThinking<'a> {
            text: &'a str,
            replay: &'a ReplayState,
        }

        RichThinking {
            text: &self.text,
            replay,
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ThinkingBlock {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum ThinkingRepresentation {
            Plain(String),
            Rich {
                text: String,
                replay: ReplayRepresentation,
            },
        }

        #[derive(Deserialize)]
        struct ReplayRepresentation {
            wire: ReplayWire,
            #[serde(default)]
            provider: Option<String>,
            #[serde(default)]
            endpoint: Option<String>,
            data: String,
            #[serde(default)]
            redacted: bool,
        }

        match ThinkingRepresentation::deserialize(deserializer)? {
            ThinkingRepresentation::Plain(text) => Ok(Self::new(text)),
            ThinkingRepresentation::Rich { text, replay } => Ok(Self {
                text,
                replay: Some(ReplayState {
                    wire: replay.wire,
                    provider: replay.provider,
                    endpoint: replay.endpoint,
                    data: replay.data,
                    redacted: replay.redacted,
                }),
            }),
        }
    }
}

/// A tool invocation requested by the model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    /// Provider-assigned call id (opaque string; matched by
    /// [`ToolResultMessage::tool_call_id`]).
    ///
    /// The value is not a packed encoding. Adapters must not split or parse
    /// it to recover other identifiers; OpenAI Responses item ids live in
    /// [`Self::item_id`].
    pub id: String,
    /// OpenAI Responses output-item id, when this call came from that wire.
    ///
    /// Other wires leave this unset. Tool results match [`Self::id`], not
    /// this field. Omitted from JSON when absent for stable serialization.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_id: Option<String>,
    /// Name of the tool to invoke.
    pub name: String,
    /// Arguments as raw JSON; validated against the tool's schema at
    /// dispatch time (`mcode-tools`).
    pub arguments: serde_json::Value,
}

impl ToolCall {
    /// Creates a tool call with an opaque provider-assigned id.
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        arguments: serde_json::Value,
    ) -> Self {
        Self {
            id: id.into(),
            item_id: None,
            name: name.into(),
            arguments,
        }
    }

    /// Sets the Responses output-item id used when replaying this call.
    pub fn with_item_id(mut self, item_id: impl Into<String>) -> Self {
        let item_id = item_id.into();
        self.item_id = (!item_id.is_empty()).then_some(item_id);
        self
    }
}

/// The outcome of executing a tool call.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolResultMessage {
    /// Id of the [`ToolCall`] this answers.
    pub tool_call_id: String,
    /// Content visible to the model.
    pub content: Vec<ContentBlock>,
    pub is_error: bool,
    /// Structured details for the UI layer only — never enters LLM context
    /// (structured diffs, cwd, …). Splitting `details` from `content`
    /// keeps tokens out of the model loop (pi's ToolResult pattern).
    pub details: Option<serde_json::Value>,
}

/// A plugin-defined message; serialized transparently (`data` passes
/// through verbatim). Serializers store it as-is to preserve plugin state
/// such as plan trackers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CustomMessage {
    /// Plugin-scoped kind discriminator, e.g. `"plugin:plan"`.
    pub kind: String,
    /// Arbitrary plugin payload, preserved verbatim.
    pub data: serde_json::Value,
}

/// Why the model stopped generating.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StopReason {
    /// Natural end of turn.
    Stop,
    /// The model wants to call tools.
    ToolUse,
    /// Output was cut off by a length/token limit.
    Length,
    /// Generation ended because of an error.
    Error,
}

/// Token usage reported by a provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

/// Binary content (base64) with its MIME type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BinaryData {
    /// Base64-encoded bytes, as expected by provider image APIs.
    pub data: String,
    /// MIME type, e.g. `"image/png"`.
    pub mime_type: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn assert_roundtrip<T>(value: &T)
    where
        T: Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug,
    {
        let json = serde_json::to_string(value).expect("serialize");
        let back: T = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(&back, value);
    }

    fn sample_tool_call() -> ToolCall {
        ToolCall::new(
            "call_abc123",
            "read",
            json!({"path": "Cargo.toml", "offset": 1}),
        )
    }

    #[test]
    fn user_message_roundtrip() {
        assert_roundtrip(&Message::User(UserMessage::text("hello")));
        assert_roundtrip(&Message::User(UserMessage {
            content: vec![
                ContentBlock::Text("describe this:".into()),
                ContentBlock::Image(BinaryData {
                    data: "aGVsbG8=".into(),
                    mime_type: "image/png".into(),
                }),
            ],
        }));
    }

    #[test]
    fn assistant_message_roundtrip_all_block_kinds() {
        for stop_reason in [
            StopReason::Stop,
            StopReason::ToolUse,
            StopReason::Length,
            StopReason::Error,
        ] {
            let msg = Message::Assistant(AssistantMessage {
                blocks: vec![
                    ContentBlock::Thinking("let me think".into()),
                    ContentBlock::Text("reading the file".into()),
                    ContentBlock::ToolCall(sample_tool_call()),
                    ContentBlock::Image(BinaryData {
                        data: "AAEC".into(),
                        mime_type: "image/jpeg".into(),
                    }),
                ],
                usage: Some(Usage {
                    input_tokens: 1200,
                    output_tokens: 42,
                }),
                stop_reason,
            });
            assert_roundtrip(&msg);
        }
    }

    #[test]
    fn assistant_message_without_usage_roundtrip() {
        assert_roundtrip(&Message::Assistant(AssistantMessage {
            blocks: vec![ContentBlock::Text("hi".into())],
            usage: None,
            stop_reason: StopReason::Stop,
        }));
    }

    #[test]
    fn thinking_block_preserves_plain_and_rich_serialized_shapes() {
        let plain = ThinkingBlock::new("plain");
        assert_eq!(serde_json::to_string(&plain).unwrap(), r#""plain""#);
        assert_roundtrip(&plain);

        let rich = ThinkingBlock::new("summary").with_replay(
            ReplayState::new(
                ReplayWire::OpenAiResponses,
                r#"{"type":"reasoning","id":"rs_1"}"#,
            )
            .with_provider("openai")
            .with_endpoint("https://api.openai.com"),
        );
        let encoded = serde_json::to_string(&rich).unwrap();
        assert!(encoded.contains("\"replay\""), "{encoded}");
        assert!(encoded.contains("open_ai_responses"), "{encoded}");
        assert!(encoded.contains("\"openai\""), "{encoded}");
        assert!(encoded.contains("\"https://api.openai.com\""), "{encoded}");
        assert_roundtrip(&rich);

        // Payloads without endpoint provenance deserialize with an unknown
        // endpoint and therefore never replay verbatim.
        let legacy = serde_json::json!({
            "text": "checked",
            "replay": {
                "wire": "open_ai_responses",
                "provider": "openai",
                "data": "{}"
            }
        });
        let legacy: ThinkingBlock = serde_json::from_value(legacy).unwrap();
        assert_eq!(legacy.replay.as_ref().unwrap().endpoint, None);

        let redacted = ThinkingBlock::new("[Reasoning redacted]").with_replay(
            ReplayState::new(ReplayWire::AnthropicMessages, "encrypted-state")
                .with_provider("anthropic")
                .with_redacted(true),
        );
        let encoded = serde_json::to_string(&redacted).unwrap();
        assert!(encoded.contains("\"redacted\":true"), "{encoded}");
        assert_roundtrip(&redacted);
    }

    #[test]
    fn same_provider_missing_producer_endpoint_does_not_replay() {
        let state = ReplayState::new(ReplayWire::OpenAiResponses, "{}").with_provider("openai");
        let domain = ReplayDomain::new(
            ReplayWire::OpenAiResponses,
            "openai",
            "https://api.openai.com",
        );
        assert!(!state.is_replayable_on(&domain));
    }

    #[test]
    fn missing_consumer_endpoint_does_not_replay() {
        let state = ReplayState::new(ReplayWire::OpenAiResponses, "{}")
            .with_provider("openai")
            .with_endpoint("https://api.openai.com");
        for endpoint in [
            "",
            "not-an-origin",
            "https://user@example.com",
            "https://api.openai.com/path",
            "https://api.openai.com/",
        ] {
            let domain = ReplayDomain::new(ReplayWire::OpenAiResponses, "openai", endpoint);
            assert!(!state.is_replayable_on(&domain));
        }
    }

    #[test]
    fn trusted_cross_profile_missing_endpoint_does_not_replay() {
        let trusted_domain = ReplayDomain::new(
            ReplayWire::AnthropicMessages,
            "anthropic-gateway",
            "https://gateway.example",
        )
        .with_trusted("anthropic");
        let missing_producer =
            ReplayState::new(ReplayWire::AnthropicMessages, "signature").with_provider("anthropic");
        assert!(!missing_producer.is_replayable_on(&trusted_domain));

        let complete_producer = missing_producer.with_endpoint("https://api.anthropic.com");
        let missing_consumer =
            ReplayDomain::new(ReplayWire::AnthropicMessages, "anthropic-gateway", "")
                .with_trusted("anthropic");
        assert!(!complete_producer.is_replayable_on(&missing_consumer));
    }

    #[test]
    fn exact_and_trusted_replay_require_matching_wire_and_valid_endpoints() {
        let endpoint = "https://api.openai.com";
        let state = ReplayState::new(ReplayWire::OpenAiResponses, "{}")
            .with_provider("openai")
            .with_endpoint(endpoint);
        let exact = ReplayDomain::new(ReplayWire::OpenAiResponses, "openai", endpoint);
        assert!(state.is_replayable_on(&exact));

        let trusted = ReplayDomain::new(
            ReplayWire::OpenAiResponses,
            "openai-gateway",
            "https://gateway.example",
        )
        .with_trusted("openai");
        assert!(state.is_replayable_on(&trusted));

        let redirected = ReplayDomain::new(
            ReplayWire::OpenAiResponses,
            "openai",
            "https://mirror.example",
        );
        assert!(!state.is_replayable_on(&redirected));
        let untrusted = ReplayDomain::new(
            ReplayWire::OpenAiResponses,
            "openai-gateway",
            "https://gateway.example",
        );
        assert!(!state.is_replayable_on(&untrusted));
        let wrong_wire = ReplayDomain::new(
            ReplayWire::AnthropicMessages,
            "openai-gateway",
            "https://gateway.example",
        )
        .with_trusted("openai");
        assert!(!state.is_replayable_on(&wrong_wire));

        let unattributed =
            ReplayState::new(ReplayWire::OpenAiResponses, "{}").with_endpoint(endpoint);
        assert!(!unattributed.is_replayable_on(&exact));
        for endpoint in ["not-an-origin", "https://api.openai.com/path"] {
            let malformed = ReplayState::new(ReplayWire::OpenAiResponses, "{}")
                .with_provider("openai")
                .with_endpoint(endpoint);
            assert!(!malformed.is_replayable_on(&exact));
        }
    }

    #[test]
    fn text_block_serializes_plain_string_and_phased_object() {
        let plain = TextBlock::new("hello");
        assert_eq!(serde_json::to_string(&plain).unwrap(), r#""hello""#);
        assert_roundtrip(&plain);

        let phased = TextBlock::new("checking the file").with_phase(AssistantPhase::Commentary);
        assert_eq!(
            serde_json::to_string(&phased).unwrap(),
            r#"{"text":"checking the file","phase":"commentary"}"#
        );
        assert_roundtrip(&phased);

        let final_answer = TextBlock::new("done")
            .with_phase(AssistantPhase::FinalAnswer)
            .with_phase(AssistantPhase::Commentary);
        assert_eq!(final_answer.phase, Some(AssistantPhase::Commentary));
        assert_eq!(
            AssistantPhase::parse("final_answer"),
            Some(AssistantPhase::FinalAnswer)
        );
        assert_eq!(AssistantPhase::parse("unknown"), None);
        assert_eq!(AssistantPhase::Commentary.as_str(), "commentary");
    }

    #[test]
    fn phased_assistant_message_roundtrips_block_order() {
        let msg = Message::Assistant(AssistantMessage {
            blocks: vec![
                ContentBlock::Text(
                    TextBlock::new("let me look").with_phase(AssistantPhase::Commentary),
                ),
                ContentBlock::ToolCall(sample_tool_call()),
                ContentBlock::Text(
                    TextBlock::new("here is the answer").with_phase(AssistantPhase::FinalAnswer),
                ),
            ],
            usage: None,
            stop_reason: StopReason::Stop,
        });
        assert_roundtrip(&msg);
    }

    #[test]
    fn tool_result_roundtrip_with_and_without_details() {
        let base = ToolResultMessage {
            tool_call_id: "call_abc123".into(),
            content: vec![ContentBlock::Text("file contents".into())],
            is_error: false,
            details: None,
        };
        assert_roundtrip(&Message::ToolResult(base.clone()));

        let with_details = ToolResultMessage {
            is_error: true,
            details: Some(json!({"cwd": "/tmp", "diff": {"added": 3, "removed": 1}})),
            ..base
        };
        assert_roundtrip(&Message::ToolResult(with_details));
    }

    #[test]
    fn tool_call_arguments_preserve_arbitrary_json() {
        let arguments = json!({"nested": {"list": [1, 2.5, null, true, "x"], "obj": {}}});
        let call = ToolCall::new("c1", "bash", arguments.clone());
        let back: ToolCall = serde_json::from_str(&serde_json::to_string(&call).unwrap()).unwrap();
        assert_eq!(back.arguments, arguments);
    }

    #[test]
    fn tool_call_ids_are_opaque_and_item_id_is_optional() {
        let call = ToolCall::new("v1:1:1:ab", "read", json!({}));
        let value = serde_json::to_value(&call).unwrap();
        assert_eq!(value["id"], "v1:1:1:ab");
        assert!(value.get("item_id").is_none());
        let back: ToolCall = serde_json::from_value(value).unwrap();
        assert_eq!(back.id, "v1:1:1:ab");
        assert_eq!(back.item_id, None);

        let with_item = call.with_item_id("fc_1");
        assert_eq!(with_item.item_id.as_deref(), Some("fc_1"));
        let old: ToolCall = serde_json::from_value(json!({
            "id": "call_1",
            "name": "read",
            "arguments": {}
        }))
        .unwrap();
        assert_eq!(old.item_id, None);
    }

    #[test]
    fn custom_message_preserves_arbitrary_json() {
        let data = json!({
            "plan": [
                {"step": 1, "title": "探索实现方案", "done": true},
                {"step": 2, "title": "write code", "done": false, "notes": null}
            ],
            "meta": {"progress": 0.5, "tags": [], "owner": serde_json::Value::Null}
        });
        let msg = Message::Custom(CustomMessage {
            kind: "plugin:plan".into(),
            data: data.clone(),
        });
        let json = serde_json::to_string(&msg).unwrap();
        let back: Message = serde_json::from_str(&json).unwrap();
        match back {
            Message::Custom(custom) => {
                assert_eq!(custom.kind, "plugin:plan");
                assert_eq!(custom.data, data);
            }
            other => panic!("expected Custom, got {other:?}"),
        }
    }

    #[test]
    fn stop_reason_and_usage_roundtrip() {
        assert_roundtrip(&StopReason::ToolUse);
        assert_roundtrip(&Usage {
            input_tokens: 7,
            output_tokens: 9,
        });
        assert_roundtrip(&BinaryData {
            data: "Zm9v".into(),
            mime_type: "application/octet-stream".into(),
        });
    }
}

// Rust guideline compliant 2026-08-26
