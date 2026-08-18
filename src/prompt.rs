//! Asking which of several things somebody meant.
//!
//! Resolution regularly ends in a near-tie: `jazzmaster` fits a Fender and a Squier about
//! equally well, and there is no honest way to choose between them from the words alone.
//! The runners-up are printed with their `--model-id` values, which is a question dressed
//! up as output — to answer it the reader has to copy an id out of a table, retype the
//! whole command and wait through a second round of lookups, all to say something they
//! could have said with one keystroke while the first answer was still on the screen.
//!
//! So ask them. A numbered list read off stdin, and nothing more: no dependency, no raw
//! mode, no cursor control. It survives ssh, a dumb terminal and a slow link, and it
//! leaves the question and the answer in the scrollback where a full-screen picker would
//! have wiped both.
//!
//! Asking is only safe under clig.dev's conditions, and each one is here because of a way
//! this could otherwise go wrong:
//!
//! - Only when stdin is a terminal. `gearprice price ... | jq` would otherwise hang
//!   forever waiting on an answer nobody is there to type.
//! - Only when prompting is allowed, so `--no-input` is believed even by a run that does
//!   happen to have a terminal — a CI job or a wrapper script knows better than we do.
//! - Never as a requirement. Every way out of here — a pipe, `--no-input`, ctrl-D, an
//!   empty line, an answer that is not on the list — lands on the same default the command
//!   would have used anyway, so declining costs nothing and `--model-id` still answers the
//!   question in advance.
//!
//! The question goes to stderr, like the spinner, so a piped report never has a prompt in
//! the middle of its JSON.

use std::io::{self, BufRead, IsTerminal, Write};

/// How many answers to take before giving up and using the default.
///
/// Somebody who has typed something off the list three times is not reading the prompt,
/// and a fourth rejection would be the program arguing with them. Giving up is cheap here
/// because the default is a real answer — the match resolution ranked first — rather than
/// a refusal to proceed.
const ATTEMPTS: usize = 3;

/// Asks which of several things the user meant.
///
/// Returns `Ok(None)` when there is nothing to ask about, when stdin is not a terminal,
/// or when prompting is disabled — every one of which means "carry on with the default".
/// `Ok(Some(index))` is an answer somebody typed, indexing into `items`.
///
/// `label` draws one row of the list. It should return something that already fits a
/// terminal row: nothing here truncates, because how wide a title may be is the report's
/// business rather than the prompt's.
///
/// The only failure is stderr itself going away. Stdin failing is treated as a question
/// left unanswered, not as a failed run.
pub fn choose<T>(
    items: &[T],
    label: impl Fn(&T) -> String,
    prompt: &str,
    allow: bool,
) -> io::Result<Option<usize>> {
    // The one place the real terminal, stdin and stderr are named. Everything that decides
    // anything lives in `ask`, which can then be run without a terminal in sight.
    let is_terminal = io::stdin().is_terminal();
    ask(
        items,
        label,
        prompt,
        allow,
        is_terminal,
        &mut io::stdin().lock(),
        &mut io::stderr().lock(),
    )
}

/// The question itself, with the terminal, the keyboard and the screen all passed in.
fn ask<T>(
    items: &[T],
    label: impl Fn(&T) -> String,
    prompt: &str,
    allow: bool,
    is_terminal: bool,
    input: &mut impl BufRead,
    output: &mut impl Write,
) -> io::Result<Option<usize>> {
    // Nobody there to answer, nobody willing to be asked, or nothing worth asking about:
    // one item is already the answer and no items is not a question. Bailing before the
    // first write is what keeps a piped run byte-for-byte what it was.
    if !allow || !is_terminal || items.len() < 2 {
        return Ok(None);
    }

    writeln!(output, "{prompt}")?;
    for (index, item) in items.iter().enumerate() {
        // Counting from one, because the list is read by a person rather than indexed by a
        // program, and marking the default so that pressing enter is a visible option
        // rather than a thing you have to know.
        let marker = if index == 0 { "  (default)" } else { "" };
        writeln!(output, "  {}) {}{marker}", index + 1, label(item))?;
    }

    let question = format!("Choose 1-{}, or press enter for 1: ", items.len());
    for attempt in 0..ATTEMPTS {
        write!(output, "{question}")?;
        // The question sits on an unfinished line, and an unflushed one is an invisible
        // one: the user would be staring at a blank terminal wondering what it wants.
        output.flush()?;

        let mut line = String::new();
        // A read that fails answers nothing, which is exactly what ctrl-D means. Since the
        // caller never needed the answer, a stray byte on stdin must not sink the run.
        let Ok(read) = input.read_line(&mut line) else {
            break;
        };
        if read == 0 {
            // ctrl-D echoes no newline, so the cursor is still sitting on the question.
            writeln!(output)?;
            break;
        }

        let answer = line.trim();
        if answer.is_empty() {
            return Ok(None);
        }
        if let Some(number) = answer
            .parse::<usize>()
            .ok()
            .filter(|number| (1..=items.len()).contains(number))
        {
            return Ok(Some(number - 1));
        }
        if attempt + 1 < ATTEMPTS {
            writeln!(output, "  {answer:?} is not one of 1-{}.", items.len())?;
        }
    }

    // Say which way it went. A prompt that vanishes without a word leaves the reader
    // unsure whether the numbers below are the ones they asked for.
    writeln!(output, "  Going with {}.", label(&items[0]))?;
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A near-tie of the kind resolution actually produces.
    const MODELS: [&str; 3] = [
        "Fender Jazzmaster",
        "Squier Jazzmaster",
        "Fender American Professional II Jazzmaster",
    ];

    /// Asks the question with the terminal, the keyboard and the screen all made up, and
    /// hands back both what was chosen and everything the user would have seen.
    fn asked(
        items: &[&str],
        typed: &str,
        is_terminal: bool,
        allow: bool,
    ) -> (Option<usize>, String) {
        let mut input = typed.as_bytes();
        let mut shown: Vec<u8> = Vec::new();
        let chosen = ask(
            items,
            |item| (*item).to_string(),
            "Several models fit that. Which did you mean?",
            allow,
            is_terminal,
            &mut input,
            &mut shown,
        )
        .unwrap();
        (chosen, String::from_utf8(shown).unwrap())
    }

    #[test]
    fn a_piped_run_never_stops_to_ask() {
        // The whole reason for the terminal check: `gearprice price ... | jq` has nobody
        // to answer it, and a question written anyway would be a question nobody sees.
        let (chosen, shown) = asked(&MODELS, "2\n", false, true);
        assert_eq!(None, chosen);
        assert!(shown.is_empty(), "a piped run wrote {shown:?}");
    }

    #[test]
    fn a_run_told_not_to_ask_stays_quiet_even_at_a_terminal() {
        // --no-input is a promise from the caller that no answer is coming, and a terminal
        // it happens to have inherited does not overrule that.
        let (chosen, shown) = asked(&MODELS, "2\n", true, false);
        assert_eq!(None, chosen);
        assert!(shown.is_empty(), "a --no-input run wrote {shown:?}");
    }

    #[test]
    fn pressing_enter_takes_the_default() {
        let (chosen, shown) = asked(&MODELS, "\n", true, true);
        assert_eq!(None, chosen);
        // And the default was named as one, so enter was an offer rather than a guess.
        assert!(shown.contains("1) Fender Jazzmaster  (default)"), "{shown}");
        assert!(shown.contains("2) Squier Jazzmaster"), "{shown}");
        assert!(
            shown.contains("Choose 1-3, or press enter for 1: "),
            "{shown}"
        );
    }

    #[test]
    fn a_number_picks_the_model_on_that_row() {
        // One-based on the screen, zero-based coming back out.
        assert_eq!(Some(0), asked(&MODELS, "1\n", true, true).0);
        assert_eq!(Some(1), asked(&MODELS, "2\n", true, true).0);
        assert_eq!(Some(2), asked(&MODELS, "3\n", true, true).0);
        // Answered without a newline, which is what a terminal sends on some shells.
        assert_eq!(Some(1), asked(&MODELS, "2", true, true).0);
        // And whitespace around it is a typing accident, not a wrong answer.
        assert_eq!(Some(1), asked(&MODELS, "  2  \n", true, true).0);
    }

    #[test]
    fn a_wrong_answer_is_asked_again_and_then_let_go() {
        let (chosen, shown) = asked(&MODELS, "9\nbanana\n0\n", true, true);
        assert_eq!(None, chosen);
        // Asked three times and no more. Without a cap this loop is a hang.
        assert_eq!(3, shown.matches("Choose 1-3").count(), "{shown}");
        assert!(shown.contains("is not one of 1-3"), "{shown}");
        assert!(shown.contains("Going with Fender Jazzmaster."), "{shown}");
    }

    #[test]
    fn a_wrong_answer_followed_by_a_right_one_still_counts() {
        let (chosen, shown) = asked(&MODELS, "7\n2\n", true, true);
        assert_eq!(Some(1), chosen);
        assert_eq!(2, shown.matches("Choose 1-3").count(), "{shown}");
    }

    #[test]
    fn ctrl_d_takes_the_default_rather_than_asking_again() {
        // Closed stdin at a terminal: the answer is never coming, and re-asking would
        // spin through the attempts against an empty read.
        let (chosen, shown) = asked(&MODELS, "", true, true);
        assert_eq!(None, chosen);
        assert_eq!(1, shown.matches("Choose 1-3").count(), "{shown}");
        assert!(shown.contains("Going with Fender Jazzmaster."), "{shown}");
    }

    #[test]
    fn one_model_or_none_is_not_a_question() {
        let (chosen, shown) = asked(&["Fender Jazzmaster"], "1\n", true, true);
        assert_eq!(None, chosen);
        assert!(
            shown.is_empty(),
            "a settled answer was put to a vote: {shown}"
        );

        let (chosen, shown) = asked(&[], "1\n", true, true);
        assert_eq!(None, chosen);
        assert!(shown.is_empty(), "an empty list was put to a vote: {shown}");
    }
}
