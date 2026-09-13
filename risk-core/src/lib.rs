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
    pub reference: Option<String>,
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
    /// The voice agent's silent classification. This is the authority for routing.
    pub risk_level: Option<RiskLevel>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RiskDecision {
    pub risk_level: RiskLevel,
    pub signals: Vec<RiskSignal>,
    pub allowed_actions: Vec<AllowedAction>,
    pub transfer_allowed: bool,
    pub proposed_action_allowed: Option<bool>,
    pub next_step: String,
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
        payment: Regex::new(r"(?i)(bank|payment|account|invoice|refund|settlement).{0,70}(change|changed|update|replace|redirect|different)|(change|changed|update|replace|redirect).{0,70}(bank|payment|account|invoice|refund|settlement)|gift[- ]cards?|(unpaid|outstanding|not paid|missing).{0,50}(subscription|invoice|payment|office)|(subscription|office).{0,40}(not paid|unpaid)|(?:pay|send|buy).{0,45}gift|pay them now|owes?\b.{0,40}\$|\$\s*\d[\d,]*.{0,40}(subscription|unpaid|not paid|owe|debt|payment)").unwrap(),
        credential: Regex::new(r"(?i)password|passcode|verification code|security code|one[- ]time code|\botp\b|\bmfa\b|multi[- ]factor|login code").unwrap(),
        remote_access: Regex::new(r"(?i)remote access|remote support|remote desktop|screen share|anydesk|teamviewer|logmein|connect(?:ed)? to (?:your|the) (?:computer|pc|system)|install.{0,35}(support|software|tool|app)").unwrap(),
        urgency: Regex::new(r"(?i)(must|have to|need to).{0,35}(today|now|immediately|right away)|urgent|immediately|right now|today only|final warning|act now|otherwise.{0,45}(suspend|close|cancel|penalty|fee|lose access)").unwrap(),
        protected_information: Regex::new(r"(?i)staff (schedule|availability|phone|number)|customer (booking|record)|invoice detail|private record|security procedure").unwrap(),
        business_caller: Regex::new(r"(?i)supplier|delivery|purchase order|invoice|provider|support department|technician|tech(?:nical)? support|microsoft|help ?desk|pos (?:provider|support|vendor)").unwrap(),
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
    let signals: BTreeSet<_> = context.signals.iter().copied().collect();
    // The agent is the judge. With no classification yet, fail closed: no transfer.
    let risk_level = context.risk_level.unwrap_or(RiskLevel::Amber);

    let allowed_actions = match context.risk_level {
        None => vec![
            AllowedAction::AskForVerification,
            AllowedAction::TakeMessage,
            AllowedAction::EndCall,
        ],
        Some(RiskLevel::Green) => vec![
            AllowedAction::AnswerEnquiry,
            AllowedAction::TakeMessage,
            AllowedAction::Transfer,
            AllowedAction::EndCall,
        ],
        Some(RiskLevel::Amber) => {
            let mut actions = vec![
                AllowedAction::AskForVerification,
                AllowedAction::TakeMessage,
                AllowedAction::EndCall,
            ];
            if amber_ready_to_transfer(context) {
                actions.insert(1, AllowedAction::Transfer);
            }
            actions
        }
        Some(RiskLevel::Red) => vec![
            AllowedAction::SafeRefusal,
            AllowedAction::BoundedDistraction,
        ],
    };
    let transfer_allowed = allowed_actions.contains(&AllowedAction::Transfer);
    let proposed_action_allowed = context
        .proposed_action
        .map(|action| allowed_actions.contains(&action));
    let next_step = match context.risk_level {
        None => "Ask exactly one permitted verification question; do not transfer until you have classified this caller.".to_owned(),
        Some(RiskLevel::Green) => "Say you will put them through, then invoke transfer_to_human in the browser demo or transfer_to_number on a phone call. Never invoke end_call. If neither transfer tool is available, stay on the line and find someone; do not ask another booking question.".to_owned(),
        Some(RiskLevel::Amber) => amber_next_step(context),
        Some(RiskLevel::Red) => red_stall_next_step(&context.transcript),
    };
    let reasons = match risk_level {
        RiskLevel::Green => {
            vec!["The agent classified this caller as an ordinary enquiry.".to_owned()]
        }
        RiskLevel::Amber => {
            vec!["The agent classified this caller as needing verification.".to_owned()]
        }
        RiskLevel::Red => vec!["The agent classified this caller as high risk.".to_owned()],
    };

    RiskDecision {
        risk_level,
        signals: signals.into_iter().collect(),
        allowed_actions,
        transfer_allowed,
        proposed_action_allowed,
        next_step,
        reasons,
    }
}

fn present(value: &Option<String>) -> bool {
    value.as_ref().is_some_and(|item| !item.trim().is_empty())
}

fn amber_ready_to_transfer(context: &CallerContext) -> bool {
    let has_name = present(&context.caller_name);
    let has_company = present(&context.claimed_company);
    let has_reference = present(&context.reference);
    let business = context.business_caller || has_company;
    has_name && (!business || (has_company && has_reference))
}

fn amber_next_step(context: &CallerContext) -> String {
    const HANDOFF: &str = "Say you will put them through, then invoke transfer_to_human in the browser demo or transfer_to_number on a phone call. Never invoke end_call. Do not discuss, confirm, or collect bank, account, or invoice numbers.";
    let business = context.business_caller || present(&context.claimed_company);
    if amber_ready_to_transfer(context) {
        HANDOFF.to_owned()
    } else if !present(&context.caller_name) {
        "Ask only for the caller's name. Do not transfer yet, and do not discuss bank details.".to_owned()
    } else if business && !present(&context.claimed_company) {
        "Ask only for the company name. Do not transfer yet, and do not discuss bank details.".to_owned()
    } else {
        "Ask only for a reference number, such as an invoice, PO, account, or ticket reference. Do not transfer yet, and do not discuss bank details.".to_owned()
    }
}

fn red_stall_next_step(transcript: &[TranscriptTurn]) -> String {
    const STAY: &str = " Keep going. Never hang up, wrap up, say goodbye, or invoke end_call. Stay on the line until the caller disconnects.";
    const STALLS: [&str; 5] = [
        "Play along with one short stall: fake a useless action, such as opening the wrong app or reading a blank screen. Do not refuse, transfer, or actually help the request.",
        "Play along with one short stall: give obviously bogus salon details, such as logging ticket Hair-000 or treating POS as the salon poodle. Do not refuse, transfer, or actually help the request.",
        "Play along with one short stall: fake slow progress, such as an install stuck at four percent. Do not refuse, transfer, or actually help the request.",
        "Play along with one short stall: mix their request with salon nonsense. Do not refuse, transfer, or actually help the request.",
        "Play along with one short stall: ask one harmless verification question only. Do not refuse, transfer, or actually help the request.",
    ];
    format!("{}{STAY}", STALLS[transcript.len() % STALLS.len()])
}

pub fn recommendation(signal: RiskSignal) -> &'static str {
    match signal {
        RiskSignal::PaymentOrAccountChange => {
            "Do not send payment or gift cards, and do not change account details; verify using the registered supplier contact."
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
            "Take the caller's name and company, then hand off; do not handle bank or invoice details yourself."
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context(
        level: RiskLevel,
        text: &str,
        proposed_action: Option<AllowedAction>,
    ) -> CallerContext {
        CallerContext {
            requested_action: text.to_owned(),
            proposed_action,
            risk_level: Some(level),
            ..Default::default()
        }
    }

    #[test]
    fn ordinary_enquiry_may_transfer() {
        let decision = evaluate(&context(
            RiskLevel::Green,
            "I would like a haircut tomorrow",
            Some(AllowedAction::Transfer),
        ));
        assert_eq!(decision.risk_level, RiskLevel::Green);
        assert!(decision.transfer_allowed);
        assert_eq!(decision.proposed_action_allowed, Some(true));
        assert!(decision.next_step.contains("put them through"));
        assert!(decision.next_step.contains("Never invoke end_call"));
    }

    #[test]
    fn named_haircut_request_is_ready_for_immediate_handoff() {
        let mut input = context(
            RiskLevel::Green,
            "My name is Jamie and I would like a haircut",
            Some(AllowedAction::Transfer),
        );
        input.caller_name = Some("Jamie".to_owned());
        let decision = evaluate(&input);
        assert_eq!(decision.risk_level, RiskLevel::Green);
        assert!(decision.transfer_allowed);
        assert_eq!(decision.proposed_action_allowed, Some(true));
        assert!(
            decision
                .next_step
                .contains("do not ask another booking question")
        );
    }

    #[test]
    fn missing_agent_classification_fails_closed() {
        let decision = evaluate(&CallerContext {
            requested_action: "I would like a haircut tomorrow".to_owned(),
            proposed_action: Some(AllowedAction::Transfer),
            ..Default::default()
        });
        assert_eq!(decision.risk_level, RiskLevel::Amber);
        assert!(!decision.transfer_allowed);
    }

    #[test]
    fn agent_green_is_not_overridden_by_scam_wording() {
        let decision = evaluate(&context(
            RiskLevel::Green,
            "Install TeamViewer so I can get remote access",
            Some(AllowedAction::Transfer),
        ));
        assert_eq!(decision.risk_level, RiskLevel::Green);
        assert!(decision.transfer_allowed);
    }

    #[test]
    fn agent_red_blocks_transfer() {
        let mut input = context(
            RiskLevel::Red,
            "Install TeamViewer so I can get remote access",
            Some(AllowedAction::Transfer),
        );
        input.known_contact = true;
        input.verified_reference = true;
        let decision = evaluate(&input);
        assert_eq!(decision.risk_level, RiskLevel::Red);
        assert!(!decision.transfer_allowed);
        assert!(
            decision
                .next_step
                .contains("Play along with one short stall")
        );
        assert!(decision.next_step.contains("wrong app"));
        assert!(!decision.allowed_actions.contains(&AllowedAction::EndCall));
        assert!(decision.next_step.contains("Never hang up"));
    }

    #[test]
    fn red_stalls_rotate_and_keep_questions_as_spice() {
        let mut input = context(
            RiskLevel::Red,
            "Install TeamViewer so I can get remote access",
            Some(AllowedAction::Transfer),
        );
        let first = evaluate(&input).next_step;
        input.transcript = vec![TranscriptTurn {
            role: "user".to_owned(),
            message: "Install it now".to_owned(),
        }];
        let second = evaluate(&input).next_step;
        input.transcript.push(TranscriptTurn {
            role: "user".to_owned(),
            message: "I need the manager".to_owned(),
        });
        let third = evaluate(&input).next_step;
        input.transcript.push(TranscriptTurn {
            role: "user".to_owned(),
            message: "Are you installing".to_owned(),
        });
        let fourth = evaluate(&input).next_step;
        input.transcript.push(TranscriptTurn {
            role: "user".to_owned(),
            message: "Transfer me".to_owned(),
        });
        let fifth = evaluate(&input).next_step;
        assert!(first.contains("wrong app"));
        assert!(first.contains("Never hang up"));
        assert!(second.contains("Hair-000"));
        assert!(third.contains("four percent"));
        assert!(fourth.contains("salon nonsense"));
        assert!(fifth.contains("harmless verification question only"));
        assert_ne!(first, second);
        assert_ne!(second, third);
    }

    #[test]
    fn supplier_stays_amber_when_the_agent_says_so() {
        let decision = evaluate(&context(
            RiskLevel::Amber,
            "Calling about tomorrow's supplier delivery",
            Some(AllowedAction::Transfer),
        ));
        assert_eq!(decision.risk_level, RiskLevel::Amber);
        assert!(!decision.transfer_allowed);
        assert_eq!(decision.proposed_action_allowed, Some(false));
        assert!(decision.next_step.contains("name"));
        assert!(decision.next_step.contains("Do not transfer yet"));
    }

    #[test]
    fn business_amber_without_a_reference_stays_on_the_line() {
        let mut input = context(
            RiskLevel::Amber,
            "it's Pruya from Loreal calling about your account",
            Some(AllowedAction::Transfer),
        );
        input.caller_name = Some("Pruya".to_owned());
        input.claimed_company = Some("Loreal".to_owned());
        input.business_caller = true;
        let decision = evaluate(&input);
        assert!(!decision.transfer_allowed);
        assert!(decision.next_step.contains("reference"));
        assert!(decision.next_step.contains("Do not transfer yet"));
    }

    #[test]
    fn screened_amber_bank_caller_may_transfer() {
        let mut input = context(
            RiskLevel::Amber,
            "Need to talk about bank details for an invoice",
            Some(AllowedAction::Transfer),
        );
        input.caller_name = Some("Morgan".to_owned());
        input.claimed_company = Some("North Star Hair Supply".to_owned());
        input.reference = Some("NS-204".to_owned());
        input.business_caller = true;
        let decision = evaluate(&input);
        assert_eq!(decision.risk_level, RiskLevel::Amber);
        assert!(decision.transfer_allowed);
        assert_eq!(decision.proposed_action_allowed, Some(true));
        assert!(decision.next_step.contains("put them through"));
        assert!(decision.next_step.contains("bank"));
    }
}
