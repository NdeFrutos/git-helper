use crate::domain::RepositoryId;

/// Identidad de una generación asociada a una sesión y a su borrador.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommitMessageRequest {
    pub session_id: RepositoryId,
    pub request_id: u64,
    pub draft_version: u64,
}

/// Resultado de validar una respuesta antes de escribir en el editor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GenerationApplyDecision {
    Apply,
    StaleRequest,
    DraftChanged,
    IndexChanged,
}

/// Impide que una respuesta antigua o inconsistente sustituya el borrador.
#[must_use]
pub fn validate_generation_result(
    request: &CommitMessageRequest,
    active_request: Option<&CommitMessageRequest>,
    current_draft_version: u64,
    expected_index_identity: &[u8],
    current_index_identity: &[u8],
) -> GenerationApplyDecision {
    if active_request != Some(request) {
        return GenerationApplyDecision::StaleRequest;
    }
    if current_draft_version != request.draft_version {
        return GenerationApplyDecision::DraftChanged;
    }
    if expected_index_identity != current_index_identity {
        return GenerationApplyDecision::IndexChanged;
    }
    GenerationApplyDecision::Apply
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> CommitMessageRequest {
        CommitMessageRequest {
            session_id: RepositoryId::new(),
            request_id: 7,
            draft_version: 3,
        }
    }

    #[test]
    fn applies_only_when_request_draft_and_index_are_unchanged() {
        let request = request();
        assert_eq!(
            validate_generation_result(&request, Some(&request), 3, b"a", b"a"),
            GenerationApplyDecision::Apply
        );
    }

    #[test]
    fn rejects_changed_staged_content_even_with_the_same_shape() {
        let request = request();
        assert_eq!(
            validate_generation_result(&request, Some(&request), 3, b"content-a", b"content-b"),
            GenerationApplyDecision::IndexChanged
        );
    }

    #[test]
    fn rejects_user_edits_and_old_or_closed_requests() {
        let request = request();
        let newer = CommitMessageRequest {
            request_id: request.request_id + 1,
            ..request.clone()
        };
        assert_eq!(
            validate_generation_result(&request, Some(&request), 4, b"a", b"a"),
            GenerationApplyDecision::DraftChanged
        );
        assert_eq!(
            validate_generation_result(&request, Some(&newer), 3, b"a", b"a"),
            GenerationApplyDecision::StaleRequest
        );
        assert_eq!(
            validate_generation_result(&request, None, 3, b"a", b"a"),
            GenerationApplyDecision::StaleRequest
        );
    }
}
