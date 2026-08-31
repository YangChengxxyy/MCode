use mcode_plugin_api::{
    FeatureTaskCompleted, FeatureTaskRequest, OperationId, ResourcesCatalogEntry,
    ResourcesCatalogRequest, ResourcesCatalogResult, ResourcesContribution,
    ResourcesContributionKind, ResourcesContributionsResult, ResourcesMedia, ResourcesMessageRole,
    ResourcesPromptArg, ResourcesPromptEntry, ResourcesPromptMessage, ResourcesPromptParam,
    ResourcesPromptResult, ResourcesReadRequest, ResourcesReadResult, ResourcesRenderPromptRequest,
    ResourcesResourceEntry, ResourcesTaskProgress, ResourcesTaskRequest, ResourcesTaskResult,
    TaskGeneration, TaskId, TaskWireError,
};

use super::{
    ResourcesValidationError, validate_resources_progress_body, validate_resources_request,
    validate_resources_result_body,
};

fn catalog_request(limit: u16) -> ResourcesTaskRequest {
    ResourcesTaskRequest::Catalog(ResourcesCatalogRequest { offset: 3, limit })
}

fn read_request() -> ResourcesTaskRequest {
    ResourcesTaskRequest::Read(ResourcesReadRequest {
        id: "guide".into(),
        offset: 2,
        max_bytes: 4,
    })
}

fn prompt_request() -> ResourcesTaskRequest {
    ResourcesTaskRequest::RenderPrompt(ResourcesRenderPromptRequest {
        id: "explain".into(),
        args: vec![ResourcesPromptArg {
            name: "topic".into(),
            value: "Rust".into(),
        }],
    })
}

#[test]
fn request_cases_have_exact_logical_charge() {
    assert_eq!(validate_resources_request(&catalog_request(1)), Ok(10));
    assert_eq!(validate_resources_request(&read_request()), Ok(25));
    assert_eq!(validate_resources_request(&prompt_request()), Ok(36));
    assert_eq!(
        validate_resources_request(&ResourcesTaskRequest::Contributions),
        Ok(4)
    );
}

#[test]
fn request_upper_and_lower_bounds_are_enforced() {
    assert_eq!(
        validate_resources_request(&catalog_request(0)),
        Err(ResourcesValidationError::InvalidArgument)
    );
    assert_eq!(
        validate_resources_request(&catalog_request(129)),
        Err(ResourcesValidationError::Limit)
    );
    let oversized_argument = ResourcesTaskRequest::RenderPrompt(ResourcesRenderPromptRequest {
        id: "explain".into(),
        args: vec![ResourcesPromptArg {
            name: "topic".into(),
            value: "x".repeat(65_537),
        }],
    });
    assert_eq!(
        validate_resources_request(&oversized_argument),
        Err(ResourcesValidationError::Limit)
    );
}

#[test]
fn semantic_charge_does_not_bypass_the_encoded_manager_wire_bound() {
    let request = |value: String| {
        ResourcesTaskRequest::RenderPrompt(ResourcesRenderPromptRequest {
            id: "explain".into(),
            args: vec![ResourcesPromptArg {
                name: "topic".into(),
                value,
            }],
        })
    };
    let ascii = request("x".repeat(32_768));
    let escaped = request("\"".repeat(32_768));
    assert_eq!(
        validate_resources_request(&ascii),
        validate_resources_request(&escaped)
    );
    let operation = OperationId::parse("render-prompt").expect("operation ID");
    let generation = TaskGeneration::new(7).expect("task generation");
    assert!(
        FeatureTaskRequest::new(operation.clone(), generation, ascii)
            .encode()
            .is_ok()
    );
    assert_eq!(
        FeatureTaskRequest::new(operation, generation, escaped).encode(),
        Err(TaskWireError::TooLarge)
    );
}

#[test]
fn legal_maximum_read_result_fails_closed_when_its_envelope_is_too_large() {
    let request = ResourcesTaskRequest::Read(ResourcesReadRequest {
        id: "guide".into(),
        offset: 0,
        max_bytes: 65_536,
    });
    let result = ResourcesTaskResult::Read(ResourcesReadResult {
        text: "x".repeat(65_536),
        next_offset: None,
    });
    assert!(validate_resources_result_body(&request, &result).is_ok());
    let completed = FeatureTaskCompleted::new(
        OperationId::parse("read").expect("operation ID"),
        TaskId::parse("task1-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa").expect("task ID"),
        TaskGeneration::new(7).expect("task generation"),
        result,
    );
    assert_eq!(completed.encode(), Err(TaskWireError::TooLarge));
}

#[test]
fn request_validation_rejects_noncanonical_ids() {
    for id in ["", "Guide", "guide_1", "-guide"] {
        let request = ResourcesTaskRequest::Read(ResourcesReadRequest {
            id: id.into(),
            offset: 0,
            max_bytes: 4,
        });
        assert_eq!(
            validate_resources_request(&request),
            Err(ResourcesValidationError::InvalidArgument)
        );
    }
}

#[test]
fn request_arguments_require_strict_order() {
    let crossed = ResourcesTaskRequest::RenderPrompt(ResourcesRenderPromptRequest {
        id: "explain".into(),
        args: vec![
            ResourcesPromptArg {
                name: "z".into(),
                value: "ok".into(),
            },
            ResourcesPromptArg {
                name: "a".into(),
                value: "also-ok".into(),
            },
        ],
    });
    assert_eq!(
        validate_resources_request(&crossed),
        Err(ResourcesValidationError::InvalidArgument)
    );
}

#[test]
fn request_text_accepts_exact_limit_and_rejects_bidi_control() {
    let exact = ResourcesTaskRequest::RenderPrompt(ResourcesRenderPromptRequest {
        id: format!("a{}", "1".repeat(127)),
        args: vec![ResourcesPromptArg {
            name: format!("a{}", "1".repeat(63)),
            value: "x".repeat(65_536),
        }],
    });
    assert!(validate_resources_request(&exact).is_ok());

    let unsafe_text = ResourcesTaskRequest::RenderPrompt(ResourcesRenderPromptRequest {
        id: "explain".into(),
        args: vec![ResourcesPromptArg {
            name: "topic".into(),
            value: "bad\u{202e}".into(),
        }],
    });
    assert_eq!(
        validate_resources_request(&unsafe_text),
        Err(ResourcesValidationError::InvalidArgument)
    );
}

#[test]
fn progress_validation_rejects_crossed_cases() {
    assert_eq!(
        validate_resources_progress_body(&catalog_request(1), ResourcesTaskProgress::Loading),
        Ok(4)
    );
    assert_eq!(
        validate_resources_progress_body(&catalog_request(1), ResourcesTaskProgress::Rendering),
        Err(ResourcesValidationError::InvalidArgument)
    );
    assert_eq!(
        validate_resources_progress_body(&prompt_request(), ResourcesTaskProgress::Rendering),
        Ok(4)
    );
}

#[test]
fn catalog_result_validates_page_shape_order_and_charge() {
    let result = ResourcesTaskResult::Catalog(ResourcesCatalogResult {
        items: vec![ResourcesCatalogEntry::Resource(ResourcesResourceEntry {
            id: "guide".into(),
            title: "Guide".into(),
            media: ResourcesMedia::Markdown,
            size_hint: Some(42),
        })],
        next_offset: Some(4),
    });
    assert_eq!(
        validate_resources_result_body(&catalog_request(1), &result),
        Ok(54)
    );

    let replayed = ResourcesTaskResult::Catalog(ResourcesCatalogResult {
        items: Vec::new(),
        next_offset: Some(3),
    });
    assert_eq!(
        validate_resources_result_body(&catalog_request(1), &replayed),
        Err(ResourcesValidationError::InvalidArgument)
    );
}

#[test]
fn catalog_result_rejects_cross_kind_duplicate_and_unsorted_parameters() {
    let duplicate = ResourcesTaskResult::Catalog(ResourcesCatalogResult {
        items: vec![
            ResourcesCatalogEntry::Resource(ResourcesResourceEntry {
                id: "guide".into(),
                title: "Guide".into(),
                media: ResourcesMedia::Text,
                size_hint: None,
            }),
            ResourcesCatalogEntry::Prompt(ResourcesPromptEntry {
                id: "guide".into(),
                title: "Guide".into(),
                params: Vec::new(),
            }),
        ],
        next_offset: None,
    });
    assert_eq!(
        validate_resources_result_body(&catalog_request(2), &duplicate),
        Err(ResourcesValidationError::InvalidArgument)
    );

    let unsorted = ResourcesTaskResult::Catalog(ResourcesCatalogResult {
        items: vec![ResourcesCatalogEntry::Prompt(ResourcesPromptEntry {
            id: "prompt".into(),
            title: "Prompt".into(),
            params: vec![
                ResourcesPromptParam {
                    name: "z".into(),
                    label: "Z".into(),
                    required: false,
                },
                ResourcesPromptParam {
                    name: "a".into(),
                    label: "A".into(),
                    required: true,
                },
            ],
        })],
        next_offset: None,
    });
    assert_eq!(
        validate_resources_result_body(&catalog_request(2), &unsorted),
        Err(ResourcesValidationError::InvalidArgument)
    );
}

#[test]
fn read_result_validates_utf8_byte_progression_and_safe_text() {
    let result = ResourcesTaskResult::Read(ResourcesReadResult {
        text: "é".into(),
        next_offset: Some(4),
    });
    assert_eq!(
        validate_resources_result_body(&read_request(), &result),
        Ok(22)
    );

    for result in [
        ResourcesTaskResult::Read(ResourcesReadResult {
            text: String::new(),
            next_offset: Some(2),
        }),
        ResourcesTaskResult::Read(ResourcesReadResult {
            text: "bad\r".into(),
            next_offset: None,
        }),
        ResourcesTaskResult::Read(ResourcesReadResult {
            text: "hello".into(),
            next_offset: None,
        }),
    ] {
        assert!(validate_resources_result_body(&read_request(), &result).is_err());
    }

    let exact_request = ResourcesTaskRequest::Read(ResourcesReadRequest {
        id: "guide".into(),
        offset: 0,
        max_bytes: 65_536,
    });
    let exact_result = ResourcesTaskResult::Read(ResourcesReadResult {
        text: "x".repeat(65_536),
        next_offset: None,
    });
    assert!(validate_resources_result_body(&exact_request, &exact_result).is_ok());
}

#[test]
fn prompt_result_enforces_request_relation_and_text_cap() {
    let prompt = ResourcesTaskResult::Prompt(ResourcesPromptResult {
        id: "explain".into(),
        messages: vec![ResourcesPromptMessage {
            role: ResourcesMessageRole::Assistant,
            text: "Done".into(),
        }],
    });
    assert_eq!(
        validate_resources_result_body(&prompt_request(), &prompt),
        Ok(31)
    );

    let crossed_id = ResourcesTaskResult::Prompt(ResourcesPromptResult {
        id: "other".into(),
        messages: Vec::new(),
    });
    assert_eq!(
        validate_resources_result_body(&prompt_request(), &crossed_id),
        Err(ResourcesValidationError::InvalidArgument)
    );

    let oversized = ResourcesTaskResult::Prompt(ResourcesPromptResult {
        id: "explain".into(),
        messages: (0..5)
            .map(|_| ResourcesPromptMessage {
                role: ResourcesMessageRole::User,
                text: "x".repeat(65_536),
            })
            .collect(),
    });
    assert_eq!(
        validate_resources_result_body(&prompt_request(), &oversized),
        Err(ResourcesValidationError::Limit)
    );
}

#[test]
fn contribution_result_enforces_charge_and_byte_order() {
    let contributions = ResourcesTaskResult::Contributions(ResourcesContributionsResult {
        items: vec![ResourcesContribution {
            id: "status".into(),
            kind: ResourcesContributionKind::Status,
        }],
    });
    assert_eq!(
        validate_resources_result_body(&ResourcesTaskRequest::Contributions, &contributions),
        Ok(22)
    );

    let unsorted = ResourcesTaskResult::Contributions(ResourcesContributionsResult {
        items: vec![
            ResourcesContribution {
                id: "z".into(),
                kind: ResourcesContributionKind::Panel,
            },
            ResourcesContribution {
                id: "a".into(),
                kind: ResourcesContributionKind::Status,
            },
        ],
    });
    assert_eq!(
        validate_resources_result_body(&ResourcesTaskRequest::Contributions, &unsorted),
        Err(ResourcesValidationError::InvalidArgument)
    );
}
