use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::providers::ProviderId;
pub use crate::providers::{ProviderCapabilities, ProviderCapability};

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ProviderUnavailability {
    NotInstalled,
    UnsupportedVersion,
    Unauthenticated,
    Unhealthy,
    QuotaBlocked,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderRoutingState {
    pub provider: ProviderId,
    pub capabilities: ProviderCapabilities,
    pub unavailable_reasons: Vec<ProviderUnavailability>,
}

impl ProviderRoutingState {
    pub fn available(provider: ProviderId, capabilities: ProviderCapabilities) -> Self {
        Self {
            provider,
            capabilities,
            unavailable_reasons: Vec::new(),
        }
    }

    pub fn unavailable(
        provider: ProviderId,
        capabilities: ProviderCapabilities,
        reason: ProviderUnavailability,
    ) -> Self {
        Self {
            provider,
            capabilities,
            unavailable_reasons: vec![reason],
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RoutingProfile {
    #[default]
    Balanced,
    BestFit,
    UsageBalance,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum TaskKind {
    Implementation,
    Review,
    Research,
    General,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RoutingReason {
    ManualOverride,
    RequiredCapabilities,
    Continuity,
    OnlyEligibleProvider,
    LeastUsed,
    DeterministicTieBreak,
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RoutingBlocker {
    Unavailable(ProviderUnavailability),
    MissingCapability(ProviderCapability),
    NotReported,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderEvaluation {
    pub provider: ProviderId,
    pub eligible: bool,
    pub blockers: Vec<RoutingBlocker>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderRank {
    pub provider: ProviderId,
    pub recent_root_runs: u64,
    pub stable_order: u8,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RoutingCriterion {
    ManualOverride { provider: ProviderId },
    EligibleProviders { providers: Vec<ProviderId> },
    RequiredCapabilities { capabilities: ProviderCapabilities },
    Continuity { provider: ProviderId },
    RankedCandidates { candidates: Vec<ProviderRank> },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RoutingDecision {
    pub provider: ProviderId,
    pub eligible_providers: Vec<ProviderId>,
    pub profile: RoutingProfile,
    pub reason: RoutingReason,
    pub override_provider: Option<ProviderId>,
    pub task_kind: TaskKind,
    pub required_capabilities: ProviderCapabilities,
    pub evaluations: Vec<ProviderEvaluation>,
    pub rationale: Vec<RoutingCriterion>,
    pub explanation: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RouteRequest {
    message: String,
    required_capabilities: ProviderCapabilities,
    candidates: Vec<ProviderRoutingState>,
    usage: Vec<(ProviderId, u64)>,
    current_provider: Option<ProviderId>,
    override_provider: Option<ProviderId>,
    profile: RoutingProfile,
}

impl RouteRequest {
    pub fn builder(message: impl Into<String>) -> RouteRequestBuilder {
        RouteRequestBuilder {
            request: Self {
                message: message.into(),
                required_capabilities: ProviderCapabilities::default(),
                candidates: Vec::new(),
                usage: Vec::new(),
                current_provider: None,
                override_provider: None,
                profile: RoutingProfile::default(),
            },
        }
    }
}

pub struct RouteRequestBuilder {
    request: RouteRequest,
}

impl RouteRequestBuilder {
    pub fn required_capabilities(
        mut self,
        capabilities: impl IntoIterator<Item = ProviderCapability>,
    ) -> Self {
        self.request.required_capabilities = capabilities.into_iter().collect();
        self
    }

    pub fn eligible(mut self, candidates: impl IntoIterator<Item = ProviderRoutingState>) -> Self {
        self.request.candidates = candidates.into_iter().collect();
        self
    }

    pub fn usage(mut self, usage: impl IntoIterator<Item = (ProviderId, u64)>) -> Self {
        self.request.usage = usage.into_iter().collect();
        self
    }

    pub fn current_provider(mut self, provider: ProviderId) -> Self {
        self.request.current_provider = Some(provider);
        self
    }

    pub fn override_provider(mut self, provider: ProviderId) -> Self {
        self.request.override_provider = Some(provider);
        self
    }

    pub fn profile(mut self, profile: RoutingProfile) -> Self {
        self.request.profile = profile;
        self
    }

    pub fn build(self) -> RouteRequest {
        self.request
    }
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum RoutingError {
    #[error("provider {provider:?} has more than one routing state")]
    DuplicateProviderState { provider: ProviderId },
    #[error("provider {provider:?} has more than one usage count")]
    DuplicateUsageCount { provider: ProviderId },
    #[error("requested provider {provider:?} is unavailable")]
    RequestedProviderUnavailable {
        provider: ProviderId,
        blockers: Vec<RoutingBlocker>,
    },
    #[error("no provider is eligible for this request")]
    NoEligibleProviders {
        evaluations: Vec<ProviderEvaluation>,
    },
}

#[derive(Default)]
pub struct Router {
    _private: (),
}

impl Router {
    pub fn route(&self, request: RouteRequest) -> Result<RoutingDecision, RoutingError> {
        validate_unique_inputs(&request)?;
        let task_kind = classify_task(&request.message);
        let mut evaluations = request
            .candidates
            .iter()
            .map(|candidate| evaluate(candidate, &request.required_capabilities))
            .collect::<Vec<_>>();
        evaluations.sort_by_key(|evaluation| provider_order(evaluation.provider));

        if let Some(provider) = request.override_provider {
            let evaluation = evaluations
                .iter()
                .find(|evaluation| evaluation.provider == provider)
                .cloned()
                .unwrap_or(ProviderEvaluation {
                    provider,
                    eligible: false,
                    blockers: vec![RoutingBlocker::NotReported],
                });
            if !evaluation.eligible {
                return Err(RoutingError::RequestedProviderUnavailable {
                    provider,
                    blockers: evaluation.blockers,
                });
            }

            return Ok(decision(
                &request,
                &evaluations,
                task_kind,
                provider,
                RoutingReason::ManualOverride,
            ));
        }

        let eligible = evaluations
            .iter()
            .filter(|evaluation| evaluation.eligible)
            .map(|evaluation| evaluation.provider)
            .collect::<Vec<_>>();
        if eligible.is_empty() {
            return Err(RoutingError::NoEligibleProviders { evaluations });
        }

        let (provider, reason) = choose_provider(&request, &evaluations, &eligible);
        Ok(decision(
            &request,
            &evaluations,
            task_kind,
            provider,
            reason,
        ))
    }
}

fn validate_unique_inputs(request: &RouteRequest) -> Result<(), RoutingError> {
    let mut providers = Vec::with_capacity(request.candidates.len());
    for candidate in &request.candidates {
        if providers.contains(&candidate.provider) {
            return Err(RoutingError::DuplicateProviderState {
                provider: candidate.provider,
            });
        }
        providers.push(candidate.provider);
    }

    providers.clear();
    for (provider, _) in &request.usage {
        if providers.contains(provider) {
            return Err(RoutingError::DuplicateUsageCount {
                provider: *provider,
            });
        }
        providers.push(*provider);
    }
    Ok(())
}

fn evaluate(
    candidate: &ProviderRoutingState,
    required: &ProviderCapabilities,
) -> ProviderEvaluation {
    let mut blockers = candidate
        .unavailable_reasons
        .iter()
        .copied()
        .map(RoutingBlocker::Unavailable)
        .collect::<Vec<_>>();
    blockers.extend(
        required
            .iter()
            .filter(|capability| !candidate.capabilities.supports(*capability))
            .map(RoutingBlocker::MissingCapability),
    );
    blockers.sort_unstable();
    blockers.dedup();
    ProviderEvaluation {
        provider: candidate.provider,
        eligible: blockers.is_empty(),
        blockers,
    }
}

fn choose_provider(
    request: &RouteRequest,
    evaluations: &[ProviderEvaluation],
    eligible: &[ProviderId],
) -> (ProviderId, RoutingReason) {
    if request.profile != RoutingProfile::UsageBalance
        && let Some(provider) = request
            .current_provider
            .filter(|provider| eligible.contains(provider))
    {
        return (provider, RoutingReason::Continuity);
    }

    if eligible.len() == 1 {
        let reason = if capabilities_excluded_available_provider(evaluations) {
            RoutingReason::RequiredCapabilities
        } else {
            RoutingReason::OnlyEligibleProvider
        };
        return (eligible[0], reason);
    }

    let ranked = ranked_candidates(request, eligible);
    let provider = ranked[0].provider;
    let lowest_usage = ranked[0].recent_root_runs;
    let tied = ranked
        .iter()
        .filter(|candidate| candidate.recent_root_runs == lowest_usage)
        .count()
        > 1;
    let reason = if tied {
        RoutingReason::DeterministicTieBreak
    } else {
        RoutingReason::LeastUsed
    };
    (provider, reason)
}

fn ranked_candidates(request: &RouteRequest, eligible: &[ProviderId]) -> Vec<ProviderRank> {
    let mut ranked = eligible
        .iter()
        .map(|provider| ProviderRank {
            provider: *provider,
            recent_root_runs: usage_for(request, *provider),
            stable_order: provider_order(*provider),
        })
        .collect::<Vec<_>>();
    ranked.sort_by_key(|candidate| (candidate.recent_root_runs, candidate.stable_order));
    ranked
}

fn capabilities_excluded_available_provider(evaluations: &[ProviderEvaluation]) -> bool {
    evaluations.iter().any(|evaluation| {
        evaluation
            .blockers
            .iter()
            .any(|blocker| matches!(blocker, RoutingBlocker::MissingCapability(_)))
            && evaluation
                .blockers
                .iter()
                .all(|blocker| matches!(blocker, RoutingBlocker::MissingCapability(_)))
    })
}

fn decision(
    request: &RouteRequest,
    evaluations: &[ProviderEvaluation],
    task_kind: TaskKind,
    provider: ProviderId,
    reason: RoutingReason,
) -> RoutingDecision {
    let eligible_providers = evaluations
        .iter()
        .filter(|evaluation| evaluation.eligible)
        .map(|evaluation| evaluation.provider)
        .collect::<Vec<_>>();
    let rationale = rationale(request, &eligible_providers, provider, reason);
    RoutingDecision {
        provider,
        eligible_providers,
        profile: request.profile,
        reason,
        override_provider: request.override_provider,
        task_kind,
        required_capabilities: request.required_capabilities.clone(),
        evaluations: evaluations.to_vec(),
        rationale,
        explanation: explanation(provider, reason),
    }
}

fn rationale(
    request: &RouteRequest,
    eligible_providers: &[ProviderId],
    provider: ProviderId,
    reason: RoutingReason,
) -> Vec<RoutingCriterion> {
    let mut rationale = Vec::new();
    if reason == RoutingReason::ManualOverride {
        rationale.push(RoutingCriterion::ManualOverride { provider });
    }
    rationale.push(RoutingCriterion::EligibleProviders {
        providers: eligible_providers.to_vec(),
    });
    if !request.required_capabilities.is_empty() {
        rationale.push(RoutingCriterion::RequiredCapabilities {
            capabilities: request.required_capabilities.clone(),
        });
    }
    match reason {
        RoutingReason::Continuity => {
            rationale.push(RoutingCriterion::Continuity { provider });
        }
        RoutingReason::LeastUsed | RoutingReason::DeterministicTieBreak => {
            rationale.push(RoutingCriterion::RankedCandidates {
                candidates: ranked_candidates(request, eligible_providers),
            });
        }
        RoutingReason::ManualOverride
        | RoutingReason::RequiredCapabilities
        | RoutingReason::OnlyEligibleProvider => {}
    }
    rationale
}

fn usage_for(request: &RouteRequest, provider: ProviderId) -> u64 {
    request
        .usage
        .iter()
        .find_map(|(candidate, count)| (*candidate == provider).then_some(*count))
        .unwrap_or(0)
}

fn provider_order(provider: ProviderId) -> u8 {
    match provider {
        ProviderId::Codex => 0,
        ProviderId::Claude => 1,
    }
}

fn explanation(provider: ProviderId, reason: RoutingReason) -> String {
    let provider = match provider {
        ProviderId::Codex => "Codex",
        ProviderId::Claude => "Claude",
    };
    let reason = match reason {
        RoutingReason::ManualOverride => "it was explicitly selected",
        RoutingReason::RequiredCapabilities => "it provides all required capabilities",
        RoutingReason::Continuity => "it preserves the current line of work",
        RoutingReason::OnlyEligibleProvider => "it is the only eligible provider",
        RoutingReason::LeastUsed => "it has the lowest recent root-run count",
        RoutingReason::DeterministicTieBreak => "it won the stable provider-order tie-break",
    };
    format!("Selected {provider} because {reason}.")
}

fn classify_task(message: &str) -> TaskKind {
    let tokens = tokens(message);
    if contains_any(&tokens, &["review", "audit", "inspect"]) {
        TaskKind::Review
    } else if contains_any(&tokens, &["research", "investigate", "compare", "explore"]) {
        TaskKind::Research
    } else if contains_any(
        &tokens,
        &[
            "implement",
            "implementation",
            "build",
            "create",
            "add",
            "fix",
            "refactor",
            "code",
            "coding",
        ],
    ) {
        TaskKind::Implementation
    } else {
        TaskKind::General
    }
}

fn contains_any(tokens: &[String], values: &[&str]) -> bool {
    tokens.iter().any(|token| values.contains(&token.as_str()))
}

fn tokens(message: &str) -> Vec<String> {
    message
        .split(|character: char| !character.is_ascii_alphanumeric())
        .filter(|token| !token.is_empty())
        .map(str::to_ascii_lowercase)
        .collect()
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use crate::providers::ProviderId;

    use super::*;

    fn healthy(provider: ProviderId) -> ProviderRoutingState {
        ProviderRoutingState::available(provider, all_capabilities())
    }

    fn unavailable(provider: ProviderId) -> ProviderRoutingState {
        ProviderRoutingState::unavailable(
            provider,
            all_capabilities(),
            ProviderUnavailability::Unhealthy,
        )
    }

    fn all_capabilities() -> ProviderCapabilities {
        ProviderCapabilities::from([
            ProviderCapability::Streaming,
            ProviderCapability::Steering,
            ProviderCapability::DeferredApproval,
            ProviderCapability::Interruption,
            ProviderCapability::Resume,
            ProviderCapability::ChildAgents,
        ])
    }

    fn request_with_override(
        provider: ProviderId,
        candidates: impl IntoIterator<Item = ProviderRoutingState>,
    ) -> RouteRequest {
        RouteRequest::builder("continue the implementation")
            .override_provider(provider)
            .eligible(candidates)
            .build()
    }

    #[test]
    fn override_beats_continuity_and_balance() {
        let request = RouteRequest::builder("continue the implementation")
            .override_provider(ProviderId::Claude)
            .current_provider(ProviderId::Codex)
            .eligible([healthy(ProviderId::Codex), healthy(ProviderId::Claude)])
            .usage([(ProviderId::Codex, 0), (ProviderId::Claude, 10)])
            .build();

        let decision = Router::default().route(request).unwrap();

        assert_eq!(decision.provider, ProviderId::Claude);
        assert_eq!(decision.reason, RoutingReason::ManualOverride);
    }

    #[test]
    fn unavailable_override_fails_instead_of_silently_switching() {
        let request = request_with_override(
            ProviderId::Claude,
            [healthy(ProviderId::Codex), unavailable(ProviderId::Claude)],
        );

        assert!(matches!(
            Router::default().route(request),
            Err(RoutingError::RequestedProviderUnavailable {
                provider: ProviderId::Claude,
                ..
            })
        ));
    }

    #[test]
    fn override_missing_a_required_capability_fails_closed() {
        let request = RouteRequest::builder("continue")
            .override_provider(ProviderId::Claude)
            .required_capabilities([ProviderCapability::DeferredApproval])
            .eligible([
                healthy(ProviderId::Codex),
                ProviderRoutingState::available(
                    ProviderId::Claude,
                    ProviderCapabilities::default(),
                ),
            ])
            .build();

        assert_eq!(
            Router::default().route(request),
            Err(RoutingError::RequestedProviderUnavailable {
                provider: ProviderId::Claude,
                blockers: vec![RoutingBlocker::MissingCapability(
                    ProviderCapability::DeferredApproval,
                )],
            })
        );
    }

    #[test]
    fn balanced_prefers_continuity_before_usage() {
        let request = RouteRequest::builder("continue the implementation")
            .current_provider(ProviderId::Codex)
            .eligible([healthy(ProviderId::Codex), healthy(ProviderId::Claude)])
            .usage([(ProviderId::Codex, 9), (ProviderId::Claude, 0)])
            .build();

        let decision = Router::default().route(request).unwrap();

        assert_eq!(decision.provider, ProviderId::Codex);
        assert_eq!(decision.reason, RoutingReason::Continuity);
    }

    #[test]
    fn required_capabilities_exclude_otherwise_healthy_providers() {
        let request = RouteRequest::builder("please wait for my approval")
            .required_capabilities([ProviderCapability::DeferredApproval])
            .eligible([
                ProviderRoutingState::available(
                    ProviderId::Codex,
                    ProviderCapabilities::from([ProviderCapability::Steering]),
                ),
                ProviderRoutingState::available(
                    ProviderId::Claude,
                    ProviderCapabilities::from([ProviderCapability::DeferredApproval]),
                ),
            ])
            .build();

        let decision = Router::default().route(request).unwrap();

        assert_eq!(decision.provider, ProviderId::Claude);
        assert_eq!(decision.eligible_providers, vec![ProviderId::Claude]);
    }

    #[test]
    fn usage_balance_selects_the_least_used_provider() {
        let request = RouteRequest::builder("continue")
            .profile(RoutingProfile::UsageBalance)
            .current_provider(ProviderId::Codex)
            .eligible([healthy(ProviderId::Codex), healthy(ProviderId::Claude)])
            .usage([(ProviderId::Codex, 7), (ProviderId::Claude, 2)])
            .build();

        let decision = Router::default().route(request).unwrap();

        assert_eq!(decision.provider, ProviderId::Claude);
        assert_eq!(decision.reason, RoutingReason::LeastUsed);
    }

    #[test]
    fn provider_order_deterministically_breaks_a_complete_tie() {
        let first = RouteRequest::builder("continue")
            .eligible([healthy(ProviderId::Claude), healthy(ProviderId::Codex)])
            .build();
        let second = RouteRequest::builder("continue")
            .eligible([healthy(ProviderId::Codex), healthy(ProviderId::Claude)])
            .build();

        let first = Router::default().route(first).unwrap();
        let second = Router::default().route(second).unwrap();

        assert_eq!(first.provider, ProviderId::Codex);
        assert_eq!(first, second);
        assert_eq!(first.reason, RoutingReason::DeterministicTieBreak);
    }

    #[test]
    fn conflicting_provider_states_are_rejected() {
        let request = RouteRequest::builder("continue")
            .eligible([
                healthy(ProviderId::Codex),
                unavailable(ProviderId::Codex),
                healthy(ProviderId::Claude),
            ])
            .build();

        assert_eq!(
            Router::default().route(request),
            Err(RoutingError::DuplicateProviderState {
                provider: ProviderId::Codex,
            })
        );
    }

    #[test]
    fn duplicate_usage_counts_are_rejected() {
        let request = RouteRequest::builder("continue")
            .eligible([healthy(ProviderId::Codex), healthy(ProviderId::Claude)])
            .usage([(ProviderId::Codex, 0), (ProviderId::Codex, 9)])
            .build();

        assert_eq!(
            Router::default().route(request),
            Err(RoutingError::DuplicateUsageCount {
                provider: ProviderId::Codex,
            })
        );
    }

    #[test]
    fn best_fit_uses_continuity_then_usage_as_a_tie_breaker() {
        let continuous = RouteRequest::builder("review this change")
            .profile(RoutingProfile::BestFit)
            .current_provider(ProviderId::Claude)
            .eligible([healthy(ProviderId::Codex), healthy(ProviderId::Claude)])
            .usage([(ProviderId::Codex, 0), (ProviderId::Claude, 8)])
            .build();
        let balanced_tie = RouteRequest::builder("review this change")
            .profile(RoutingProfile::BestFit)
            .eligible([healthy(ProviderId::Codex), healthy(ProviderId::Claude)])
            .usage([(ProviderId::Codex, 5), (ProviderId::Claude, 1)])
            .build();

        assert_eq!(
            Router::default().route(continuous).unwrap().provider,
            ProviderId::Claude
        );
        assert_eq!(
            Router::default().route(balanced_tie).unwrap().provider,
            ProviderId::Claude
        );
    }

    #[test]
    fn provider_name_in_message_never_overrides_structured_continuity() {
        let request = RouteRequest::builder("Please use Claude for this turn.")
            .current_provider(ProviderId::Codex)
            .eligible([healthy(ProviderId::Codex), healthy(ProviderId::Claude)])
            .build();

        let decision = Router::default().route(request).unwrap();

        assert_eq!(decision.provider, ProviderId::Codex);
        assert_eq!(decision.reason, RoutingReason::Continuity);
    }

    #[test]
    fn negated_provider_name_never_selects_that_provider() {
        let request = RouteRequest::builder("Don't use Claude for this turn.")
            .current_provider(ProviderId::Codex)
            .eligible([healthy(ProviderId::Codex), healthy(ProviderId::Claude)])
            .build();

        let decision = Router::default().route(request).unwrap();

        assert_eq!(decision.provider, ProviderId::Codex);
        assert_eq!(decision.reason, RoutingReason::Continuity);
    }

    #[test]
    fn no_suitable_provider_reports_each_rejection() {
        let request = RouteRequest::builder("continue")
            .required_capabilities([ProviderCapability::Steering])
            .eligible([
                unavailable(ProviderId::Codex),
                ProviderRoutingState::available(
                    ProviderId::Claude,
                    ProviderCapabilities::default(),
                ),
            ])
            .build();

        let error = Router::default().route(request).unwrap_err();

        let RoutingError::NoEligibleProviders { evaluations } = error else {
            panic!("expected an eligibility error");
        };
        assert_eq!(evaluations.len(), 2);
        assert!(evaluations.iter().all(|evaluation| !evaluation.eligible));
    }

    #[test]
    fn decision_records_task_kind_exclusions_and_ordered_rationale() {
        let request = RouteRequest::builder("Please review this patch")
            .required_capabilities([ProviderCapability::DeferredApproval])
            .eligible([
                unavailable(ProviderId::Codex),
                ProviderRoutingState::available(
                    ProviderId::Claude,
                    ProviderCapabilities::from([ProviderCapability::DeferredApproval]),
                ),
            ])
            .build();

        let decision = Router::default().route(request).unwrap();

        assert_eq!(decision.task_kind, TaskKind::Review);
        assert!(
            decision
                .required_capabilities
                .supports(ProviderCapability::DeferredApproval)
        );
        assert_eq!(decision.evaluations.len(), 2);
        assert_eq!(decision.evaluations[0].provider, ProviderId::Codex);
        assert!(!decision.evaluations[0].eligible);
        assert_eq!(
            decision.rationale,
            vec![
                RoutingCriterion::EligibleProviders {
                    providers: vec![ProviderId::Claude],
                },
                RoutingCriterion::RequiredCapabilities {
                    capabilities: ProviderCapabilities::from([
                        ProviderCapability::DeferredApproval,
                    ]),
                },
            ]
        );
    }

    #[test]
    fn usage_balance_rationale_does_not_claim_continuity_contributed() {
        let request = RouteRequest::builder("continue")
            .profile(RoutingProfile::UsageBalance)
            .current_provider(ProviderId::Codex)
            .eligible([healthy(ProviderId::Codex), healthy(ProviderId::Claude)])
            .usage([(ProviderId::Codex, 8), (ProviderId::Claude, 2)])
            .build();

        let decision = Router::default().route(request).unwrap();

        assert_eq!(decision.provider, ProviderId::Claude);
        assert_eq!(
            decision.rationale,
            vec![
                RoutingCriterion::EligibleProviders {
                    providers: vec![ProviderId::Codex, ProviderId::Claude],
                },
                RoutingCriterion::RankedCandidates {
                    candidates: vec![
                        ProviderRank {
                            provider: ProviderId::Claude,
                            recent_root_runs: 2,
                            stable_order: 1,
                        },
                        ProviderRank {
                            provider: ProviderId::Codex,
                            recent_root_runs: 8,
                            stable_order: 0,
                        },
                    ],
                },
            ]
        );

        let value = serde_json::to_value(decision).unwrap();
        assert_eq!(
            value["rationale"][1]["rankedCandidates"]["candidates"],
            serde_json::json!([
                {"provider": "claude", "recentRootRuns": 2, "stableOrder": 1},
                {"provider": "codex", "recentRootRuns": 8, "stableOrder": 0},
            ])
        );
    }

    #[test]
    fn full_tie_serializes_every_compared_value_in_stable_order() {
        let decision = Router::default()
            .route(
                RouteRequest::builder("continue")
                    .eligible([healthy(ProviderId::Claude), healthy(ProviderId::Codex)])
                    .usage([(ProviderId::Claude, 4), (ProviderId::Codex, 4)])
                    .build(),
            )
            .unwrap();

        assert_eq!(decision.provider, ProviderId::Codex);
        assert_eq!(decision.reason, RoutingReason::DeterministicTieBreak);
        let value = serde_json::to_value(decision).unwrap();
        assert_eq!(
            value["rationale"][1]["rankedCandidates"]["candidates"],
            serde_json::json!([
                {"provider": "codex", "recentRootRuns": 4, "stableOrder": 0},
                {"provider": "claude", "recentRootRuns": 4, "stableOrder": 1},
            ])
        );
    }

    #[test]
    fn deserialized_capabilities_are_sorted_and_deduplicated() {
        let capabilities: ProviderCapabilities =
            serde_json::from_value(serde_json::json!(["steering", "streaming", "steering"]))
                .unwrap();

        assert_eq!(
            serde_json::to_value(capabilities).unwrap(),
            serde_json::json!(["streaming", "steering"])
        );
    }

    #[test]
    fn equivalent_unavailability_orders_produce_identical_errors() {
        let state = |reasons| ProviderRoutingState {
            provider: ProviderId::Codex,
            capabilities: ProviderCapabilities::default(),
            unavailable_reasons: reasons,
        };
        let first = RouteRequest::builder("continue")
            .eligible([state(vec![
                ProviderUnavailability::QuotaBlocked,
                ProviderUnavailability::Unhealthy,
                ProviderUnavailability::QuotaBlocked,
            ])])
            .build();
        let second = RouteRequest::builder("continue")
            .eligible([state(vec![
                ProviderUnavailability::Unhealthy,
                ProviderUnavailability::QuotaBlocked,
            ])])
            .build();

        assert_eq!(
            Router::default().route(first),
            Router::default().route(second)
        );
    }

    #[test]
    fn task_signals_are_observable_without_provider_quality_bias() {
        let cases = [
            ("implement the feature", TaskKind::Implementation),
            ("review this patch", TaskKind::Review),
            ("research the available options", TaskKind::Research),
            ("what happened?", TaskKind::General),
        ];

        for (message, expected_kind) in cases {
            let decision = Router::default()
                .route(
                    RouteRequest::builder(message)
                        .eligible([healthy(ProviderId::Claude), healthy(ProviderId::Codex)])
                        .usage([(ProviderId::Codex, 0), (ProviderId::Claude, 0)])
                        .build(),
                )
                .unwrap();

            assert_eq!(decision.task_kind, expected_kind);
            assert_eq!(decision.provider, ProviderId::Codex);
            assert_eq!(decision.reason, RoutingReason::DeterministicTieBreak);
        }
    }

    fn generated_state(
        provider: ProviderId,
        available: bool,
        supports_steering: bool,
    ) -> ProviderRoutingState {
        let capabilities = if supports_steering {
            ProviderCapabilities::from([ProviderCapability::Steering])
        } else {
            ProviderCapabilities::default()
        };
        if available {
            ProviderRoutingState::available(provider, capabilities)
        } else {
            ProviderRoutingState::unavailable(
                provider,
                capabilities,
                ProviderUnavailability::Unauthenticated,
            )
        }
    }

    proptest! {
        #[test]
        fn generated_routes_never_choose_an_ineligible_provider(
            codex_available in any::<bool>(),
            claude_available in any::<bool>(),
            codex_steers in any::<bool>(),
            claude_steers in any::<bool>(),
            require_steering in any::<bool>(),
            codex_usage in 0_u64..100,
            claude_usage in 0_u64..100,
        ) {
            let mut builder = RouteRequest::builder("continue")
                .eligible([
                    generated_state(ProviderId::Codex, codex_available, codex_steers),
                    generated_state(ProviderId::Claude, claude_available, claude_steers),
                ])
                .usage([
                    (ProviderId::Codex, codex_usage),
                    (ProviderId::Claude, claude_usage),
                ]);
            if require_steering {
                builder = builder.required_capabilities([ProviderCapability::Steering]);
            }

            if let Ok(decision) = Router::default().route(builder.build()) {
                let eligible = match decision.provider {
                    ProviderId::Codex => codex_available && (!require_steering || codex_steers),
                    ProviderId::Claude => claude_available && (!require_steering || claude_steers),
                };
                prop_assert!(eligible);
            }
        }

        #[test]
        fn generated_identical_requests_are_deterministic(
            codex_available in any::<bool>(),
            claude_available in any::<bool>(),
            codex_usage in any::<u64>(),
            claude_usage in any::<u64>(),
            use_balance in any::<bool>(),
        ) {
            let profile = if use_balance {
                RoutingProfile::UsageBalance
            } else {
                RoutingProfile::Balanced
            };
            let request = RouteRequest::builder("implement the next change")
                .profile(profile)
                .current_provider(ProviderId::Claude)
                .eligible([
                    generated_state(ProviderId::Codex, codex_available, true),
                    generated_state(ProviderId::Claude, claude_available, true),
                ])
                .usage([
                    (ProviderId::Codex, codex_usage),
                    (ProviderId::Claude, claude_usage),
                ])
                .build();

            prop_assert_eq!(
                Router::default().route(request.clone()),
                Router::default().route(request),
            );
        }

        #[test]
        fn generated_unique_input_permutations_route_identically(
            codex_available in any::<bool>(),
            claude_available in any::<bool>(),
            codex_steers in any::<bool>(),
            claude_steers in any::<bool>(),
            codex_usage in any::<u64>(),
            claude_usage in any::<u64>(),
        ) {
            let codex = generated_state(ProviderId::Codex, codex_available, codex_steers);
            let claude = generated_state(ProviderId::Claude, claude_available, claude_steers);
            let first = RouteRequest::builder("continue")
                .required_capabilities([ProviderCapability::Steering])
                .eligible([codex.clone(), claude.clone()])
                .usage([
                    (ProviderId::Codex, codex_usage),
                    (ProviderId::Claude, claude_usage),
                ])
                .build();
            let second = RouteRequest::builder("continue")
                .required_capabilities([ProviderCapability::Steering])
                .eligible([claude, codex])
                .usage([
                    (ProviderId::Claude, claude_usage),
                    (ProviderId::Codex, codex_usage),
                ])
                .build();

            prop_assert_eq!(
                Router::default().route(first),
                Router::default().route(second),
            );
        }

        #[test]
        fn generated_eligible_override_is_always_honored(
            override_claude in any::<bool>(),
            other_available in any::<bool>(),
            override_usage in any::<u64>(),
            other_usage in any::<u64>(),
        ) {
            let (requested, other) = if override_claude {
                (ProviderId::Claude, ProviderId::Codex)
            } else {
                (ProviderId::Codex, ProviderId::Claude)
            };
            let request = RouteRequest::builder("continue")
                .override_provider(requested)
                .current_provider(other)
                .required_capabilities([ProviderCapability::Steering])
                .eligible([
                    generated_state(requested, true, true),
                    generated_state(other, other_available, true),
                ])
                .usage([(requested, override_usage), (other, other_usage)])
                .build();

            let decision = Router::default().route(request).unwrap();
            prop_assert_eq!(decision.provider, requested);
            prop_assert_eq!(decision.reason, RoutingReason::ManualOverride);
        }
    }
}
