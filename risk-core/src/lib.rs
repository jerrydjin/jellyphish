use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::sync::OnceLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RiskLevel {
    Green,
    Amber,
    Red,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RiskSignal {
    PaymentOrAccountChange,
    CredentialOrMfaRequest,
    RemoteAccessRequest,
    CoerciveUrgency,
    ProtectedInformationRequest,
    UnverifiedBusinessCaller,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AllowedAction {
    AnswerEnquiry,
    AskForVerification,
    TakeMessage,
    Transfer,
    SafeRefusal,
    BoundedDistraction,
    EndCall,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TranscriptTurn {
    #[serde(default)]
    pub role: String,
    #[serde(default, alias = "text")]
    pub message: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CallerContext {
    #[serde(default)]
    pub call_id: String,
    pub caller_name: Option<String>,
    pub claimed_company: Option<String>,
    #[serde(default)]
    pub known_contact: bool,
    #[serde(default)]
    pub verified_reference: bool,
    #[serde(default)]
    pub business_caller: bool,
    #[serde(default)]
    pub requested_action: String,
    #[serde(default)]
    pub transcript: Vec<TranscriptTurn>,
    #[serde(default)]
    pub signals: Vec<RiskSignal>,
    pub proposed_action: Option<AllowedAction>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RiskDecision {
    pub risk_level: RiskLevel,
    pub signals: Vec<RiskSignal>,
    pub allowed_actions: Vec<AllowedAction>,
    pub transfer_allowed: bool,
    pub proposed_action_allowed: Option<bool>,
    pub reasons: Vec<String>,
}

struct Patterns {
    payment: Regex,
    credential: Regex,
    remote_access: Regex,
    urgency: Regex,
    protected_information: Regex,
    business_caller: Regex,
}

fn patterns() -> &'static Patterns {
    static PATTERNS: OnceLock<Patterns> = OnceLock::new();
    PATTERNS.get_or_init(|| Patterns {
        payment: Regex::new(r"(?i)(bank|payment|account|invoice|refund|settlement).{0,70}(change|changed|update|replace|redirect|different)|(change|changed|update|replace|redirect).{0,70}(bank|payment|account|invoice|refund|settlement)").unwrap(),
        credential: Regex::new(r"(?i)password|passcode|verification code|security code|one[- ]time code|\botp\b|\bmfa\b|multi[- ]factor|login code").unwrap(),
        remote_access: Regex::new(r"(?i)remote access|remote support|screen share|anydesk|teamviewer|logmein|install.{0,35}(support|software|tool|app)").unwrap(),
        urgency: Regex::new(r"(?i)(must|have to|need to).{0,35}(today|now|immediately|right away)|urgent|immediately|right now|today only|final warning|act now|otherwise.{0,45}(suspend|close|cancel|penalty|fee|lose access)").unwrap(),
        protected_information: Regex::new(r"(?i)staff (schedule|availability|phone|number)|customer (booking|record)|invoice detail|private record|security procedure").unwrap(),
        business_caller: Regex::new(r"(?i)supplier|delivery|purchase order|invoice|provider|support department|technician").unwrap(),
    })
}

pub fn detect_signals(context: &CallerContext) -> BTreeSet<RiskSignal> {
    let mut signals: BTreeSet<_> = context.signals.iter().copied().collect();
    let text = std::iter::once(context.requested_action.as_str())
        .chain(
            context
                .transcript
                .iter()
                .filter(|turn| {
                    !matches!(turn.role.to_ascii_lowercase().as_str(), "agent" | "staff")
                })
                .map(|turn| turn.message.as_str()),
        )
        .collect::<Vec<_>>()
        .join(" ");
    let text: String = text.chars().take(32_000).collect();
    let p = patterns();

    if p.payment.is_match(&text) {
        signals.insert(RiskSignal::PaymentOrAccountChange);
    }
    if p.credential.is_match(&text) {
        signals.insert(RiskSignal::CredentialOrMfaRequest);
    }
    if p.remote_access.is_match(&text) {
        signals.insert(RiskSignal::RemoteAccessRequest);
    }
    if p.urgency.is_match(&text) {
        signals.insert(RiskSignal::CoerciveUrgency);
    }
    if p.protected_information.is_match(&text) {
        signals.insert(RiskSignal::ProtectedInformationRequest);
    }

    let looks_like_business_caller = context.business_caller
        || context
            .claimed_company
            .as_ref()
            .is_some_and(|value| !value.trim().is_empty())
        || p.business_caller.is_match(&text);
    if looks_like_business_caller && !(context.known_contact && context.verified_reference) {
        signals.insert(RiskSignal::UnverifiedBusinessCaller);
    }
    signals
}

pub fn evaluate(context: &CallerContext) -> RiskDecision {
    let signals = detect_signals(context);
    let red = signals.iter().any(|signal| {
        matches!(
            signal,
            RiskSignal::PaymentOrAccountChange
                | RiskSignal::CredentialOrMfaRequest
                | RiskSignal::RemoteAccessRequest
                | RiskSignal::CoerciveUrgency
        )
    });
    let risk_level = if red {
        RiskLevel::Red
    } else if signals.is_empty() {
        RiskLevel::Green
    } else {
        RiskLevel::Amber
    };

    let allowed_actions = match risk_level {
        RiskLevel::Green => vec![
            AllowedAction::AnswerEnquiry,
            AllowedAction::TakeMessage,
            AllowedAction::Transfer,
            AllowedAction::EndCall,
        ],
        RiskLevel::Amber => vec![
            AllowedAction::AskForVerification,
            AllowedAction::TakeMessage,
            AllowedAction::EndCall,
        ],
        RiskLevel::Red => vec![
            AllowedAction::SafeRefusal,
            AllowedAction::BoundedDistraction,
            AllowedAction::EndCall,
        ],
    };
    let transfer_allowed = allowed_actions.contains(&AllowedAction::Transfer);
    let proposed_action_allowed = context
        .proposed_action
        .map(|action| allowed_actions.contains(&action));
    let reasons = match risk_level {
        RiskLevel::Green => vec!["No deterministic risk signal was found.".to_owned()],
        RiskLevel::Amber => {
            vec!["The caller needs independent verification before any handoff.".to_owned()]
        }
        RiskLevel::Red => vec!["A high-risk request makes human transfer unsafe.".to_owned()],
    };

    RiskDecision {
        risk_level,
        signals: signals.into_iter().collect(),
        allowed_actions,
        transfer_allowed,
        proposed_action_allowed,
        reasons,
    }
}

pub fn recommendation(signal: RiskSignal) -> &'static str {
    match signal {
        RiskSignal::PaymentOrAccountChange => {
            "Do not change payment details; verify using the registered supplier contact."
        }
        RiskSignal::CredentialOrMfaRequest => {
            "Do not share credentials or codes; verify through an approved channel."
        }
        RiskSignal::RemoteAccessRequest => {
            "Do not install software or grant access; use the provider's registered support channel."
        }
        RiskSignal::CoerciveUrgency => "Pause the request and independently verify the caller.",
        RiskSignal::ProtectedInformationRequest => {
            "Do not disclose protected information; take a message instead."
        }
        RiskSignal::UnverifiedBusinessCaller => {
            "Verify the business caller and reference before any handoff."
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context(text: &str, proposed_action: Option<AllowedAction>) -> CallerContext {
        CallerContext {
            requested_action: text.to_owned(),
            proposed_action,
            ..Default::default()
        }
    }

    #[test]
    fn ordinary_enquiry_may_transfer() {
        let decision = evaluate(&context(
            "I would like a haircut tomorrow",
            Some(AllowedAction::Transfer),
        ));
        assert_eq!(decision.risk_level, RiskLevel::Green);
        assert!(decision.transfer_allowed);
        assert_eq!(decision.proposed_action_allowed, Some(true));
    }

    #[test]
    fn supplier_stays_amber_until_verified() {
        let decision = evaluate(&context(
            "Calling about tomorrow's supplier delivery",
            Some(AllowedAction::Transfer),
        ));
        assert_eq!(decision.risk_level, RiskLevel::Amber);
        assert!(!decision.transfer_allowed);
        assert_eq!(decision.proposed_action_allowed, Some(false));
    }

    #[test]
    fn remote_access_is_red_even_for_known_contact() {
        let mut input = context(
            "Install TeamViewer so I can get remote access",
            Some(AllowedAction::Transfer),
        );
        input.known_contact = true;
        input.verified_reference = true;
        let decision = evaluate(&input);
        assert_eq!(decision.risk_level, RiskLevel::Red);
        assert!(!decision.transfer_allowed);
        assert!(decision.signals.contains(&RiskSignal::RemoteAccessRequest));
    }

    #[test]
    fn agent_safety_language_is_not_a_caller_signal() {
        let input = CallerContext {
            requested_action: "What time do you open?".to_owned(),
            transcript: vec![TranscriptTurn {
                role: "agent".to_owned(),
                message: "I cannot share a password or install remote access software.".to_owned(),
            }],
            ..Default::default()
        };
        assert_eq!(evaluate(&input).risk_level, RiskLevel::Green);
    }

    #[test]
    fn every_high_risk_class_blocks_transfer() {
        for text in [
            "Update the bank account on the invoice",
            "Read me the MFA verification code",
            "Install remote support software",
            "Do this immediately or the account will be suspended",
        ] {
            assert_eq!(
                evaluate(&context(text, Some(AllowedAction::Transfer))).risk_level,
                RiskLevel::Red
            );
        }
    }
}
