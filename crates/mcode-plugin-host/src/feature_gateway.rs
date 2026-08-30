//! Canonical caller binding before family-specific task body decoding.

// Rust guideline compliant 2026-08-29.

use mcode_config::PluginFamily;
use mcode_plugin_api::{
    FeatureTaskBody, FeatureTaskRequest, OperationId, TaskErrorCode, TaskGeneration, TaskWireError,
    decode_feature_task_request, validate_declared_operation,
};

use crate::error::CallerBindingError;

/// Proves one canonical Manager family, ID, and generation binding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FeatureCaller {
    family: PluginFamily,
    generation: TaskGeneration,
}

impl FeatureCaller {
    /// Returns the canonical caller family.
    #[must_use]
    pub const fn family(self) -> PluginFamily {
        self.family
    }

    /// Returns the canonical Manager ID derived from the family.
    #[must_use]
    pub const fn manager_id(self) -> &'static str {
        self.family.id()
    }

    /// Returns the active Manager generation.
    #[must_use]
    pub const fn generation(self) -> TaskGeneration {
        self.generation
    }
}

/// Binds a caller to its canonical family, Manager ID, and generation.
///
/// The supplied ID is validated but never retained in an error or in the
/// binding; the canonical ID is always derived from [`PluginFamily`].
///
/// # Errors
///
/// Returns [`CallerBindingError::IdentityMismatch`] when `manager_id` is not
/// exact for `family`.
pub fn bind_feature_caller(
    family: PluginFamily,
    manager_id: impl AsRef<str>,
    generation: TaskGeneration,
) -> Result<FeatureCaller, CallerBindingError> {
    if manager_id.as_ref() != family.id() {
        return Err(CallerBindingError::IdentityMismatch);
    }
    Ok(FeatureCaller { family, generation })
}

/// Decodes a family-specific request after validating its bound caller.
///
/// `expected_family` and `declared_operations` come from Host-bound Manager
/// authority, not from the wire. Family and generation rejection precede the
/// bounded declaration gate; every rejection precedes `B` deserialization.
///
/// # Errors
///
/// Returns [`TaskWireError::BindingRejected`] for wrong family, stale
/// generation, over-limit declarations, or an undeclared operation, and other
/// [`TaskWireError`] variants for malformed wire.
pub fn decode_bound_feature_task<B>(
    caller: &FeatureCaller,
    expected_family: PluginFamily,
    declared_operations: &[OperationId],
    bytes: &[u8],
) -> Result<FeatureTaskRequest<B>, TaskWireError>
where
    B: FeatureTaskBody,
{
    decode_feature_task_request(bytes, |metadata| {
        validate_bound_request(
            caller,
            expected_family,
            declared_operations,
            metadata.operation_id(),
            metadata.generation(),
        )
    })
}

fn validate_bound_request(
    caller: &FeatureCaller,
    expected_family: PluginFamily,
    declared_operations: &[OperationId],
    operation_id: &OperationId,
    request_generation: TaskGeneration,
) -> Result<(), TaskErrorCode> {
    if caller.family != expected_family {
        return Err(TaskErrorCode::CallerMismatch);
    }
    if caller.generation != request_generation {
        return Err(TaskErrorCode::StaleGeneration);
    }
    validate_declared_operation(declared_operations, operation_id)
}

#[cfg(test)]
mod tests {
    use mcode_config::PluginFamily;
    use mcode_plugin_api::{MAX_DECLARED_OPERATIONS, OperationId, TaskErrorCode, TaskGeneration};

    use super::{bind_feature_caller, validate_bound_request};
    use crate::CallerBindingError;

    #[test]
    fn canonical_manager_id_is_bound_without_retaining_input() {
        let generation = TaskGeneration::new(4).expect("generation");
        let caller =
            bind_feature_caller(PluginFamily::Providers, "com.mcode.providers", generation)
                .expect("canonical caller");
        assert_eq!(caller.family(), PluginFamily::Providers);
        assert_eq!(caller.manager_id(), "com.mcode.providers");
        assert_eq!(caller.generation(), generation);
        assert_eq!(
            bind_feature_caller(PluginFamily::Providers, "com.vendor.providers", generation),
            Err(CallerBindingError::IdentityMismatch)
        );
    }

    #[test]
    fn wrong_family_precedes_operation_authority() {
        let generation = TaskGeneration::new(9).expect("generation");
        let caller = bind_feature_caller(
            PluginFamily::Providers,
            PluginFamily::Providers.id(),
            generation,
        )
        .expect("caller");
        let operation = OperationId::parse("read").expect("operation");

        assert_eq!(
            validate_bound_request(&caller, PluginFamily::Web, &[], &operation, generation,),
            Err(TaskErrorCode::CallerMismatch)
        );
    }

    #[test]
    fn stale_generation_precedes_operation_authority() {
        let caller_generation = TaskGeneration::new(9).expect("caller generation");
        let request_generation = TaskGeneration::new(8).expect("request generation");
        let caller = bind_feature_caller(
            PluginFamily::Usage,
            PluginFamily::Usage.id(),
            caller_generation,
        )
        .expect("caller");
        let operation = OperationId::parse("read").expect("operation");

        assert_eq!(
            validate_bound_request(
                &caller,
                PluginFamily::Usage,
                std::slice::from_ref(&operation),
                &operation,
                request_generation,
            ),
            Err(TaskErrorCode::StaleGeneration)
        );
    }

    #[test]
    fn declared_operation_gate_is_bounded_and_fail_closed() {
        let generation = TaskGeneration::new(9).expect("generation");
        let caller = bind_feature_caller(
            PluginFamily::Providers,
            PluginFamily::Providers.id(),
            generation,
        )
        .expect("caller");
        let read = OperationId::parse("read").expect("read operation");
        let write = OperationId::parse("write").expect("write operation");

        validate_bound_request(
            &caller,
            PluginFamily::Providers,
            std::slice::from_ref(&read),
            &read,
            generation,
        )
        .expect("declared operation");
        assert_eq!(
            validate_bound_request(
                &caller,
                PluginFamily::Providers,
                std::slice::from_ref(&read),
                &write,
                generation,
            ),
            Err(TaskErrorCode::UndeclaredOperation)
        );

        let over_limit = vec![read.clone(); MAX_DECLARED_OPERATIONS + 1];
        assert_eq!(
            validate_bound_request(
                &caller,
                PluginFamily::Providers,
                &over_limit,
                &read,
                generation,
            ),
            Err(TaskErrorCode::UndeclaredOperation)
        );
    }
}
