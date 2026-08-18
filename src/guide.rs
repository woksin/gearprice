//! The walkthrough `gearprice guide` prints.
//!
//! `--help` lists what the flags are; it cannot say what anybody should type. Somebody
//! who has just installed this wants to know how to find out what a guitar is worth, and
//! the answer to that is four commands and one caveat rather than eleven subcommands and
//! forty flags.
//!
//! Written as tasks in the order they come up: what is this worth, is this listing any
//! good, which version am I holding, and what do the numbers actually mean.

use std::io::{self, Write};

/// Writes the guide, in the terminal's width where that can be measured.
pub fn write(out: &mut impl Write, colour: bool) -> io::Result<()> {
    let bold = |text: &str| {
        if colour {
            format!("\x1b[1m{text}\x1b[0m")
        } else {
            text.to_string()
        }
    };
    let dim = |text: &str| {
        if colour {
            format!("\x1b[2m{text}\x1b[0m")
        } else {
            text.to_string()
        }
    };
    let command = |text: &str| {
        if colour {
            format!("\x1b[36m{text}\x1b[0m")
        } else {
            text.to_string()
        }
    };

    writeln!(
        out,
        "{}",
        bold("gearprice — what gear is worth, and what to pay")
    )?;
    writeln!(out)?;
    writeln!(
        out,
        "Everything comes from Reverb's public marketplace. No account, no API key."
    )?;
    writeln!(out)?;

    writeln!(out, "{}", bold("What is this worth?"))?;
    writeln!(
        out,
        "  {}",
        command("gearprice \"gibson les paul standard\"")
    )?;
    writeln!(
        out,
        "  {}",
        dim("Spelling is forgiven: \"gibsen les pual standrd\" finds the same guitar.")
    )?;
    writeln!(out)?;
    writeln!(
        out,
        "  The answer is the first three lines. {} is what people have actually",
        bold("Sold")
    )?;
    writeln!(
        out,
        "  paid. {} is what sellers want, which runs well above it. {} is how",
        bold("Asking"),
        bold("Room")
    )?;
    writeln!(out, "  much sellers typically come down.")?;
    writeln!(out)?;

    writeln!(out, "{}", bold("Is this listing any good?"))?;
    writeln!(
        out,
        "  {}",
        command("gearprice deal https://reverb.com/item/95521465-…")
    )?;
    writeln!(
        out,
        "  {}",
        dim("Paste the address from the browser. It judges that one listing and")
    )?;
    writeln!(
        out,
        "  {}",
        dim("tells you the offer that would be the usual discount off its ask.")
    )?;
    writeln!(out)?;

    writeln!(out, "{}", bold("Which version am I looking at?"))?;
    writeln!(out, "  {}", command("gearprice models \"marshall major\""))?;
    writeln!(
        out,
        "  {}",
        dim("Eras and variants sell for very different money. Marshall's guitar")
    )?;
    writeln!(
        out,
        "  {}",
        dim("Major and bass Major are both 200-watt \"Majors\"; one goes for half")
    )?;
    writeln!(
        out,
        "  {}",
        dim("again what the other does. Then price one exactly:")
    )?;
    writeln!(out, "  {}", command("gearprice --model-id 137981"))?;
    writeln!(out)?;

    writeln!(out, "{}", bold("Am I paying too much?"))?;
    writeln!(
        out,
        "  {}",
        command("gearprice \"les paul standard\" --asking 1900")
    )?;
    writeln!(
        out,
        "  {}",
        dim("Says which band 1900 falls in, against what people paid rather than")
    )?;
    writeln!(out, "  {}", dim("against what other sellers are asking."))?;
    writeln!(out)?;

    writeln!(out, "{}", bold("Buying from abroad"))?;
    writeln!(
        out,
        "  {}",
        command("gearprice \"jazzmaster\" --currency NOK --ships-to NO")
    )?;
    writeln!(
        out,
        "  {}",
        dim("Drops every seller who will not post to you, and prices the rest")
    )?;
    writeln!(
        out,
        "  {}",
        dim("delivered. Of 108,000 used electric guitars, 31,000 ship to Norway.")
    )?;
    writeln!(out)?;
    writeln!(
        out,
        "  Tired of retyping that? {}",
        command("gearprice config --example > ~/.config/gearprice/config.toml")
    )?;
    writeln!(out)?;

    writeln!(out, "{}", bold("Watching something over time"))?;
    writeln!(
        out,
        "  {}",
        command("gearprice track \"les paul standard\"")
    )?;
    writeln!(
        out,
        "  {}",
        dim("Records what it costs today. Run it again in a month and it says what")
    )?;
    writeln!(
        out,
        "  {}",
        dim("moved. Nothing else can tell you that later — Reverb will not.")
    )?;
    writeln!(out)?;

    writeln!(out, "{}", bold("Reading the numbers honestly"))?;
    writeln!(
        out,
        "  · Sold prices are completed Reverb sales. Private and shop sales"
    )?;
    writeln!(
        out,
        "    elsewhere are invisible, so a rare piece has a thin record."
    )?;
    writeln!(
        out,
        "  · Where nothing has sold, bands fall back to asking prices and say so."
    )?;
    writeln!(
        out,
        "  · A market of six listings has no meaningful ninetieth percentile, and"
    )?;
    writeln!(
        out,
        "    gearprice tells you when that is what you are looking at."
    )?;
    writeln!(
        out,
        "  · \"Listed for\" is the giveaway: what is priced right sells and leaves,"
    )?;
    writeln!(
        out,
        "    so anything sitting for months is priced above the market."
    )?;
    writeln!(out)?;

    writeln!(
        out,
        "{}",
        dim("Full reference: gearprice --help, or any command with --help.")
    )?;
    writeln!(
        out,
        "{}",
        dim("Every report also comes as --format json or csv.")
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_guide_leads_with_the_thing_people_install_this_to_do() {
        let mut written = Vec::new();
        write(&mut written, false).unwrap();
        let text = String::from_utf8(written).unwrap();

        // The first task, before any flag reference.
        let worth = text
            .find("What is this worth?")
            .expect("the opening question");
        let reference = text.find("Full reference").expect("the reference pointer");
        assert!(worth < reference);

        // Every command it names is one that exists.
        for command in [
            "gearprice deal",
            "gearprice models",
            "gearprice track",
            "gearprice config --example",
            "--asking",
            "--ships-to",
            "--model-id",
        ] {
            assert!(text.contains(command), "the guide never mentions {command}");
        }
    }

    #[test]
    fn the_caveats_are_in_it_rather_than_left_for_the_reader_to_discover() {
        let mut written = Vec::new();
        write(&mut written, false).unwrap();
        let text = String::from_utf8(written).unwrap();
        // The three things that make a number mean less than it looks like.
        assert!(text.contains("completed Reverb sales"));
        assert!(text.contains("fall back to asking prices"));
        assert!(text.contains("ninetieth percentile"));
    }

    #[test]
    fn a_run_without_colour_carries_no_escape_codes() {
        let mut plain = Vec::new();
        write(&mut plain, false).unwrap();
        assert!(!String::from_utf8(plain).unwrap().contains('\x1b'));

        let mut coloured = Vec::new();
        write(&mut coloured, true).unwrap();
        assert!(String::from_utf8(coloured).unwrap().contains('\x1b'));
    }
}
