//! Live smoke test against the real OpenAI API. Not run by default
//! (CI has no keys); run explicitly with:
//!
//! ```text
//! OPENAI_API_KEY=sk-... cargo test -p mcode-llm --test live_openai -- --ignored --nocapture
//! ```

use mcode_llm::provider::{Provider, Request};
use mcode_llm::{CancellationToken, ProfileProvider, generic_openai_profile};

#[tokio::test]
#[ignore = "hits the real OpenAI API; requires OPENAI_API_KEY"]
async fn streams_a_text_completion_from_openai() {
    let Ok(api_key) = std::env::var("OPENAI_API_KEY") else {
        eprintln!("skipping: OPENAI_API_KEY is not set");
        return;
    };

    let provider = ProfileProvider::new(generic_openai_profile(), api_key).expect("live profile");
    let request = Request::new("gpt-4o-mini")
        .with_system_prompt("Answer with exactly one word.")
        .with_message(mcode_core::Message::User(mcode_core::UserMessage::text(
            "Say pong",
        )));

    let cancel = CancellationToken::new();
    let stream = provider
        .stream(&request, cancel)
        .await
        .expect("stream starts");
    let message = stream
        .into_final_message()
        .await
        .expect("live stream completes");

    let text: String = message
        .blocks
        .iter()
        .filter_map(|block| match block {
            mcode_core::message::ContentBlock::Text(text) => Some(text.text.as_str()),
            _ => None,
        })
        .collect();
    assert!(!text.trim().is_empty(), "expected non-empty completion");
    assert!(message.usage.is_some(), "expected usage with include_usage");
    eprintln!("live completion: {text}");
}
