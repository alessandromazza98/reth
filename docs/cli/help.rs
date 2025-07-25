#!/usr/bin/env -S cargo +nightly -Zscript
---
[package]
edition = "2021"

[dependencies]
clap = { version = "4", features = ["derive"] }
regex = "1"
---
use clap::Parser;
use regex::Regex;
use std::{
    borrow::Cow,
    fmt, fs, io,
    iter::once,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    str,
    sync::LazyLock,
};

const README: &str = r#"import Summary from './SUMMARY.mdx';

# CLI Reference

Automatically-generated CLI reference from `--help` output.

<Summary />
"#;
const TRIM_LINE_END_MARKDOWN: bool = true;

/// Lazy static regex to avoid recompiling the same regex pattern multiple times.
macro_rules! regex {
    ($re:expr) => {{
        static RE: LazyLock<Regex> =
            LazyLock::new(|| Regex::new($re).expect("Failed to compile regex pattern"));
        &*RE
    }};
}

/// Generate markdown files from help output of commands
#[derive(Parser, Debug)]
#[command(about, long_about = None)]
struct Args {
    /// Root directory
    #[arg(long, default_value_t = String::from("."))]
    root_dir: String,

    /// Indentation for the root SUMMARY.mdx file
    #[arg(long, default_value_t = 2)]
    root_indentation: usize,

    /// Output directory
    #[arg(long)]
    out_dir: PathBuf,

    /// Whether to add a README.md file
    #[arg(long)]
    readme: bool,

    /// Whether to update the root SUMMARY.mdx file
    #[arg(long)]
    root_summary: bool,

    /// Print verbose output
    #[arg(short, long)]
    verbose: bool,

    /// Commands to generate markdown for.
    #[arg(required = true, num_args = 1..)]
    commands: Vec<PathBuf>,
}

fn write_file(file_path: &Path, content: &str) -> io::Result<()> {
    let content = if TRIM_LINE_END_MARKDOWN {
        content.lines().map(|line| line.trim_end()).collect::<Vec<_>>().join("\n")
    } else {
        content.to_string()
    };
    fs::write(file_path, content)
}

fn main() -> io::Result<()> {
    let args = Args::parse();
    debug_assert!(args.commands.len() >= 1);

    let out_dir = args.out_dir;
    fs::create_dir_all(&out_dir)?;

    let mut todo_iter: Vec<Cmd> = args
        .commands
        .iter()
        .rev() // reverse to keep the order (pop)
        .map(Cmd::new)
        .collect();
    let mut output = Vec::new();

    // Iterate over all commands and their subcommands.
    while let Some(cmd) = todo_iter.pop() {
        let (new_subcmds, stdout) = get_entry(&cmd)?;
        if args.verbose && !new_subcmds.is_empty() {
            println!("Found subcommands for \"{}\": {:?}", cmd.command_name(), new_subcmds);
        }
        // Add new subcommands to todo_iter (so that they are processed in the correct order).
        for subcmd in new_subcmds.into_iter().rev() {
            let new_subcmds: Vec<_> = cmd.subcommands.iter().cloned().chain(once(subcmd)).collect();

            todo_iter.push(Cmd { cmd: cmd.cmd, subcommands: new_subcmds });
        }
        output.push((cmd, stdout));
    }

    // Generate markdown files.
    for (cmd, stdout) in &output {
        cmd_markdown(&out_dir, cmd, stdout)?;
    }

    // Generate SUMMARY.mdx.
    let summary: String = output
        .iter()
        .map(|(cmd, _)| cmd_summary(cmd, 0))
        .chain(once("\n".to_string()))
        .collect();

    println!("Writing SUMMARY.mdx to \"{}\"", out_dir.to_string_lossy());
    write_file(&out_dir.clone().join("SUMMARY.mdx"), &summary)?;

    // Generate README.md.
    if args.readme {
        let path = &out_dir.join("README.mdx");
        if args.verbose {
            println!("Writing README.mdx to \"{}\"", path.to_string_lossy());
        }
        write_file(path, README)?;
    }

    // Generate root SUMMARY.mdx.
    if args.root_summary {
        let root_summary: String = output
            .iter()
            .map(|(cmd, _)| cmd_summary(cmd, args.root_indentation))
            .collect();

        let path = Path::new(args.root_dir.as_str());
        if args.verbose {
            println!("Updating root summary in \"{}\"", path.to_string_lossy());
        }
        // TODO: This is where we update the cli reference sidebar.ts
        update_root_summary(path, &root_summary)?;
    }

    Ok(())
}

/// Returns the subcommands and help output for a command.
fn get_entry(cmd: &Cmd) -> io::Result<(Vec<String>, String)> {
    let output = Command::new(cmd.cmd)
        .args(&cmd.subcommands)
        .arg("--help")
        .env("NO_COLOR", "1")
        .env("COLUMNS", "100")
        .env("LINES", "10000")
        .stdout(Stdio::piped())
        .output()?;

    if !output.status.success() {
        let stderr = str::from_utf8(&output.stderr).unwrap_or("Failed to parse stderr as UTF-8");
        return Err(io::Error::new(
            io::ErrorKind::Other,
            format!("Command \"{}\" failed:\n{}", cmd, stderr),
        ));
    }

    let stdout = str::from_utf8(&output.stdout)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?
        .to_string();

    // Parse subcommands from the help output
    let subcmds = parse_sub_commands(&stdout);

    Ok((subcmds, stdout))
}

/// Returns a list of subcommands from the help output of a command.
fn parse_sub_commands(s: &str) -> Vec<String> {
    // This regex matches lines starting with two spaces, followed by the subcommand name.
    let re = regex!(r"^  (\S+)");

    s.split("Commands:")
        .nth(1) // Get the part after "Commands:"
        .map(|commands_section| {
            commands_section
                .lines()
                .take_while(|line| !line.starts_with("Options:") && !line.starts_with("Arguments:"))
                .filter_map(|line| {
                    re.captures(line).and_then(|cap| cap.get(1).map(|m| m.as_str().to_string()))
                })
                .filter(|cmd| cmd != "help")
                .map(String::from)
                .collect()
        })
        .unwrap_or_default() // Return an empty Vec if "Commands:" was not found
}

/// Writes the markdown for a command to out_dir.
fn cmd_markdown(out_dir: &Path, cmd: &Cmd, stdout: &str) -> io::Result<()> {
    let out = format!("# {}\n\n{}", cmd, help_markdown(cmd, stdout));

    let out_path = out_dir.join(cmd.to_string().replace(" ", "/"));
    fs::create_dir_all(out_path.parent().unwrap())?;
    write_file(&out_path.with_extension("mdx"), &out)?;

    Ok(())
}

/// Returns the markdown for a command's help output.
fn help_markdown(cmd: &Cmd, stdout: &str) -> String {
    let (description, s) = parse_description(stdout);
    format!(
        "{}\n\n```bash\n$ {} --help\n```\n```txt\n{}\n```",
        description,
        cmd,
        preprocess_help(s.trim())
    )
}

/// Splits the help output into a description and the rest.
fn parse_description(s: &str) -> (&str, &str) {
    match s.find("Usage:") {
        Some(idx) => {
            let description = s[..idx].trim().lines().next().unwrap_or("");
            (description, &s[idx..])
        }
        None => ("", s),
    }
}

/// Returns the summary for a command and its subcommands.
fn cmd_summary(cmd: &Cmd, indent: usize) -> String {
    let cmd_s = cmd.to_string();
    let cmd_path = cmd_s.replace(" ", "/");
    let indent_string = " ".repeat(indent + (cmd.subcommands.len() * 2));
    format!("{}- [`{}`](/cli/{})\n", indent_string, cmd_s, cmd_path)
}

/// Overwrites the root SUMMARY.mdx file with the generated content.
fn update_root_summary(root_dir: &Path, root_summary: &str) -> io::Result<()> {
    let summary_file = root_dir.join("vocs/docs/pages/cli/SUMMARY.mdx");
    println!("Overwriting {}", summary_file.display());

    // Simply write the root summary content to the file
    write_file(&summary_file, root_summary)
}

/// Preprocesses the help output of a command.
fn preprocess_help(s: &str) -> Cow<'_, str> {
    static REPLACEMENTS: LazyLock<Vec<(Regex, &str)>> = LazyLock::new(|| {
        let patterns: &[(&str, &str)] = &[
            // Remove the user-specific paths.
            (r"default: /.*/reth", "default: <CACHE_DIR>"),
            // Remove the commit SHA and target architecture triple or fourth
            //  rustup available targets:
            //    aarch64-apple-darwin
            //    x86_64-unknown-linux-gnu
            //    x86_64-pc-windows-gnu
            (
                r"default: reth/.*-[0-9A-Fa-f]{6,10}/([_\w]+)-(\w+)-(\w+)(-\w+)?",
                "default: reth/<VERSION>-<SHA>/<ARCH>",
            ),
            // Remove the OS
            (r"default: reth/.*/\w+", "default: reth/<VERSION>/<OS>"),
            // Remove rpc.max-tracing-requests default value
            (
                r"(rpc.max-tracing-requests <COUNT>\n.*\n.*\n.*\n.*\n.*)\[default: \d+\]",
                r"$1[default: <NUM CPU CORES-2>]",
            ),
            // Handle engine.max-proof-task-concurrency dynamic default
            (
                r"(engine\.max-proof-task-concurrency.*)\[default: \d+\]",
                r"$1[default: <DYNAMIC: CPU cores * 8>]",
            ),
            // Handle engine.reserved-cpu-cores dynamic default
            (
                r"(engine\.reserved-cpu-cores.*)\[default: \d+\]",
                r"$1[default: <DYNAMIC: min(2, CPU cores)>]",
            ),
        ];
        patterns
            .iter()
            .map(|&(re, replace_with)| (Regex::new(re).expect(re), replace_with))
            .collect()
    });

    let mut s = Cow::Borrowed(s);
    for (re, replacement) in REPLACEMENTS.iter() {
        if let Cow::Owned(result) = re.replace_all(&s, *replacement) {
            s = Cow::Owned(result);
        }
    }
    s
}

#[derive(Hash, Debug, PartialEq, Eq)]
struct Cmd<'a> {
    /// path to binary (e.g. ./target/debug/reth)
    cmd: &'a Path,
    /// subcommands (e.g. [db, stats])
    subcommands: Vec<String>,
}

impl<'a> Cmd<'a> {
    fn command_name(&self) -> &str {
        self.cmd.file_name().and_then(|os_str| os_str.to_str()).expect("Expect valid command")
    }

    fn new(cmd: &'a PathBuf) -> Self {
        Self { cmd, subcommands: Vec::new() }
    }
}

impl<'a> fmt::Display for Cmd<'a> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.command_name())?;
        if !self.subcommands.is_empty() {
            write!(f, " {}", self.subcommands.join(" "))?;
        }
        Ok(())
    }
}
