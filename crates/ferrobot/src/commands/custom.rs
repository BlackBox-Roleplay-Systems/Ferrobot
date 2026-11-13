//! Custom commands, as SecondBot's CustomCommandsService: a name the operator chooses (its
//! trigger) that runs a list of existing commands, its steps, with the caller's arguments put in
//! the places marked `[C_ARG_1]`, `[C_ARG_2]`… Asked by any way a command comes (the HTTP API,
//! IM, Discord), under the trigger exactly as written (SecondBot's dictionary is case-sensitive);
//! a built-in command's name always wins, as SecondBot looks those up first.
//!
//! As SecondBot's: the caller must give at least the arguments the command declares (more are
//! left out), the steps run one after the other with no pause, a step naming another custom
//! command fails ("Custom command chaining is not allowed"), and the answer counts the steps that
//! ran ("<trigger>: ran with 2 commands issued"), or says they all failed. A step may name a reply
//! target (`#|#target`), which gets that step's answer.
//!
//! Each step goes through the normal dispatcher ([`super::step`]): its own argument count, its own
//! rate limit when it changes things, and the caller's own rights ([`super::Caller`]), so a custom
//! command never lets anyone run what they could not run by asking for it directly.
//!
//! Deliberate differences, against injection through the arguments: each step's text is read once,
//! at start-up, into its command, its arguments and its reply target, and an argument is put
//! inside one of those as plain text. SecondBot puts the arguments in the step's text and reads
//! it after, so an argument holding `~#~` added arguments, one holding `#|#` named a reply target,
//! and one holding `[C_ARG_2]` was replaced in turn by the second argument; here none of them do.
//! Also: a step that is missing, a trigger that is missing or given twice, or one that is a
//! built-in command's name, stop the bot at start-up (SecondBot runs `Logoff` for a missing step
//! and names a missing trigger "exitplease"); and the answer's `Key` is true when a step ran
//! (SecondBot answers false whatever happened).

use super::{Caller, Ctx};
use crate::api::signing;

/// What a placeholder starts with: `[C_ARG_<n>]`, n from 1.
const PLACEHOLDER: &str = "[C_ARG_";

/// One step: a command, its arguments and its reply target, placeholders still in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Step {
    pub command: String,
    pub args: Vec<String>,
    pub reply: Option<String>,
}

impl Step {
    /// A step as SignedCommand reads a command's text: `command|||arg~#~arg#|#reply target`, or
    /// the JSON form `{"cmd": …, "args": […], "reply": …}`. A signature or a time in it is left
    /// out (SecondBot runs the steps unsigned). `None` when it names no command.
    pub fn parse(text: &str) -> Option<Step> {
        let u = signing::unpack(text)?;
        Some(Step {
            command: u.command,
            args: u.args,
            reply: u.reply,
        })
    }

    /// The step with these arguments in its placeholders.
    fn with(&self, args: &[String]) -> Step {
        Step {
            command: substitute(&self.command, args),
            args: self.args.iter().map(|a| substitute(a, args)).collect(),
            reply: self.reply.as_deref().map(|r| substitute(r, args)),
        }
    }

    /// The placeholders the step uses.
    pub fn placeholders(&self) -> impl Iterator<Item = usize> + '_ {
        std::iter::once(self.command.as_str())
            .chain(self.args.iter().map(String::as_str))
            .chain(self.reply.as_deref())
            .flat_map(|text| placeholders(text).map(|(_, _, n)| n))
    }
}

/// A custom command (SecondBot's CustomCommand).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CustomCommand {
    pub trigger: String,
    /// The arguments it takes, at least.
    pub args: usize,
    pub steps: Vec<Step>,
}

impl CustomCommand {
    /// The steps with the caller's arguments in; `None` when there are too few. Only the first
    /// `args` arguments fill placeholders, as SecondBot's loop: a higher one stays as it is.
    pub fn expand(&self, args: &[String]) -> Option<Vec<Step>> {
        let args = args.get(..self.args)?;
        Some(self.steps.iter().map(|s| s.with(args)).collect())
    }
}

/// The custom commands the settings define.
#[derive(Debug, Default)]
pub struct CustomCommands {
    commands: Vec<CustomCommand>,
}

impl CustomCommands {
    pub fn new(commands: Vec<CustomCommand>) -> Self {
        CustomCommands { commands }
    }

    /// The custom command of this trigger, as written (SecondBot's HasCommand).
    pub fn find(&self, name: &str) -> Option<&CustomCommand> {
        self.commands.iter().find(|c| c.trigger == name)
    }
}

/// Each placeholder in `text`: where it starts, where it ends, and its number. Only `[C_ARG_<n>]`
/// with n written as SecondBot writes it (no sign, no leading zero) counts.
fn placeholders(text: &str) -> impl Iterator<Item = (usize, usize, usize)> + '_ {
    let mut from = 0;
    std::iter::from_fn(move || {
        while let Some(at) = text[from..].find(PLACEHOLDER).map(|i| from + i) {
            let digits_at = at + PLACEHOLDER.len();
            from = digits_at;
            // no `]` after it: no placeholder further on either
            let len = text[digits_at..].find(']')?;
            let digits = &text[digits_at..digits_at + len];
            if let Ok(n) = digits.parse::<usize>()
                && n >= 1
                && digits == n.to_string()
            {
                let end = digits_at + len + 1;
                from = end;
                return Some((at, end, n));
            }
        }
        None
    })
}

/// `text` with each placeholder of an argument given replaced by that argument, in one pass: an
/// argument put in is never read again for placeholders.
fn substitute(text: &str, args: &[String]) -> String {
    let mut out = String::with_capacity(text.len());
    let mut copied = 0;
    for (start, end, n) in placeholders(text) {
        if let Some(arg) = args.get(n - 1) {
            out.push_str(&text[copied..start]);
            out.push_str(arg);
            copied = end;
        }
    }
    out.push_str(&text[copied..]);
    out
}

/// CustomCommandsService.RunCommand: the steps, in order, each through the dispatcher with the
/// caller's rights.
pub async fn run(
    ctx: &Ctx,
    caller: Caller,
    custom: &CustomCommand,
    args: &[String],
) -> (bool, String) {
    let Some(steps) = custom.expand(args) else {
        return (false, "Incorrect number of args".into());
    };
    let mut issued = 0usize;
    for (n, step) in steps.iter().enumerate() {
        let (ran, answer) = super::step(ctx, caller, &step.command, &step.args).await;
        log::info!(
            "custom command {}: step {} {} ({} args): {}",
            custom.trigger,
            n + 1,
            step.command,
            step.args.len(),
            if ran { "issued" } else { "failed" }
        );
        if ran {
            issued += 1;
        }
        // RunCommand answers to the reply target after a built-in command only
        if let (Some(target), Some(command)) = (&step.reply, super::known(&step.command)) {
            crate::smart_reply::send(ctx, target, &answer, command).await;
        }
    }
    if issued == 0 {
        return (
            false,
            format!("all custom commands have failed for: {}", custom.trigger),
        );
    }
    (
        true,
        format!("{}: ran with {issued} commands issued", custom.trigger),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_owned()).collect()
    }

    fn step(text: &str) -> Step {
        Step::parse(text).expect("a step")
    }

    #[test]
    fn steps_are_read_as_signed_commands_are() {
        assert_eq!(
            step("Say|||0~#~hello [C_ARG_1]#|#7"),
            Step {
                command: "Say".into(),
                args: strings(&["0", "hello [C_ARG_1]"]),
                reply: Some("7".into()),
            }
        );
        assert_eq!(
            step(r#"{"cmd":"IM","args":["[C_ARG_1]","hi"]}"#),
            Step {
                command: "IM".into(),
                args: strings(&["[C_ARG_1]", "hi"]),
                reply: None,
            }
        );
        // a signature or a time is left out
        assert_eq!(step("Hello@@@abc;|;123"), step("Hello"));
        assert!(Step::parse("").is_none());
        assert!(Step::parse("|||a").is_none());
    }

    #[test]
    fn arguments_fill_their_placeholders() {
        let c = CustomCommand {
            trigger: "greet".into(),
            args: 2,
            steps: vec![
                step("Say|||0~#~Hi [C_ARG_1], from [C_ARG_2] ([C_ARG_3])"),
                step("IM|||[C_ARG_1]~#~[C_ARG_2][C_ARG_2]#|#[C_ARG_2]"),
            ],
        };
        assert_eq!(c.expand(&strings(&["a"])), None, "too few");
        let steps = c.expand(&strings(&["a", "b", "c"])).expect("enough");
        // the third is not declared: its placeholder stays, as SecondBot's loop leaves it
        assert_eq!(steps[0].args, strings(&["0", "Hi a, from b ([C_ARG_3])"]));
        assert_eq!(steps[1].args, strings(&["a", "bb"]));
        assert_eq!(steps[1].reply.as_deref(), Some("b"));
        let used: Vec<usize> = c.steps[0].placeholders().collect();
        assert_eq!(used, vec![1, 2, 3]);
    }

    #[test]
    fn arguments_cannot_change_a_steps_shape() {
        let c = CustomCommand {
            trigger: "tell".into(),
            args: 2,
            steps: vec![step("Say|||0~#~[C_ARG_1]"), step("[C_ARG_2]|||x")],
        };
        let sneaky = "a~#~b#|#http://example.invalid/@@@x;|;1|||c [C_ARG_2]";
        let steps = c.expand(&strings(&[sneaky, "Name"])).expect("enough");
        // one argument, as given: no more arguments, no reply target, no second substitution
        assert_eq!(steps[0].command, "Say");
        assert_eq!(steps[0].args, strings(&["0", sneaky]));
        assert_eq!(steps[0].reply, None);
        assert_eq!(steps[1].command, "Name");
    }

    #[test]
    fn only_well_written_placeholders() {
        let args = strings(&["x"]);
        assert_eq!(substitute("[C_ARG_1]", &args), "x");
        assert_eq!(
            substitute("[C_ARG_01] [C_ARG_+1] [C_ARG_0]", &args),
            "[C_ARG_01] [C_ARG_+1] [C_ARG_0]"
        );
        assert_eq!(substitute("[C_ARG_1 [C_ARG_1]", &args), "[C_ARG_1 x");
        assert_eq!(substitute("[C_ARG_[C_ARG_1]]", &args), "[C_ARG_x]");
        assert_eq!(
            substitute("[C_ARG_99999999999999999999999]", &args),
            "[C_ARG_99999999999999999999999]"
        );
        assert_eq!(substitute("é[C_ARG_1]é", &args), "éxé");
    }

    #[test]
    fn triggers_are_case_sensitive() {
        let all = CustomCommands::new(vec![CustomCommand {
            trigger: "greet".into(),
            args: 0,
            steps: vec![step("Hello")],
        }]);
        assert!(all.find("greet").is_some());
        assert!(all.find("Greet").is_none());
    }
}
