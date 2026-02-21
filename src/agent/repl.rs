use reedline::{
    default_emacs_keybindings, ColumnarMenu, Completer, DefaultPrompt, DefaultPromptSegment,
    EditCommand, Emacs, ExternalPrinter, FileBackedHistory, KeyCode, KeyModifiers, MenuBuilder,
    Reedline, ReedlineEvent, ReedlineMenu, Signal, Span, Suggestion,
};
use std::path::PathBuf;
use tokio::sync::mpsc;

/// Events sent from the reedline thread to the async agent loop.
pub enum ReplEvent {
    Line(String),
}

/// Slash-command tab completer.
#[derive(Clone)]
struct SlashCommandCompleter {
    commands: Vec<String>,
}

impl SlashCommandCompleter {
    fn new() -> Self {
        Self {
            commands: vec![
                "/help".into(),
                "/quit".into(),
                "/exit".into(),
                "/clear".into(),
                "/new".into(),
            ],
        }
    }
}

impl Completer for SlashCommandCompleter {
    fn complete(&mut self, line: &str, pos: usize) -> Vec<Suggestion> {
        if !line.starts_with('/') {
            return vec![];
        }
        let prefix = &line[..pos];
        self.commands
            .iter()
            .filter(|cmd| cmd.starts_with(prefix) && *cmd != prefix)
            .map(|cmd| Suggestion {
                value: cmd.clone(),
                description: None,
                style: None,
                extra: None,
                span: Span::new(0, pos),
                append_whitespace: false,
                match_indices: None,
            })
            .collect()
    }
}

pub struct Repl;

impl Repl {
    /// Spawn the reedline editor on a dedicated OS thread.
    ///
    /// Returns a receiver for user input events and an `ExternalPrinter`
    /// that can print output without corrupting the prompt.
    pub fn spawn(
        history_path: PathBuf,
    ) -> (mpsc::UnboundedReceiver<ReplEvent>, ExternalPrinter<String>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let printer = ExternalPrinter::default();
        let printer_clone = printer.clone();

        std::thread::spawn(move || {
            run_reedline(tx, printer_clone, history_path);
        });

        (rx, printer)
    }
}

fn run_reedline(tx: mpsc::UnboundedSender<ReplEvent>, printer: ExternalPrinter<String>, history_path: PathBuf) {
    let history = Box::new(
        FileBackedHistory::with_file(1000, history_path)
            .expect("failed to open history file"),
    );

    let completer = Box::new(SlashCommandCompleter::new());
    let completion_menu = Box::new(ColumnarMenu::default().with_name("completion_menu"));

    let mut keybindings = default_emacs_keybindings();
    keybindings.add_binding(
        KeyModifiers::NONE,
        KeyCode::Tab,
        ReedlineEvent::UntilFound(vec![
            ReedlineEvent::Menu("completion_menu".to_string()),
            ReedlineEvent::MenuNext,
        ]),
    );
    // Ctrl+D sends EOF (Ctrl+C is already handled by reedline as Signal::CtrlC)
    keybindings.add_binding(
        KeyModifiers::CONTROL,
        KeyCode::Char('d'),
        ReedlineEvent::Edit(vec![EditCommand::Clear]),
    );

    let edit_mode = Box::new(Emacs::new(keybindings));

    let prompt = DefaultPrompt::new(
        DefaultPromptSegment::Basic("\u{1f980} > ".to_string()),
        DefaultPromptSegment::Empty,
    );

    let mut editor = Reedline::create()
        .with_history(history)
        .with_completer(completer)
        .with_menu(ReedlineMenu::EngineCompleter(completion_menu))
        .with_edit_mode(edit_mode)
        .with_external_printer(printer);

    loop {
        match editor.read_line(&prompt) {
            Ok(Signal::Success(line)) => {
                let trimmed = line.trim().to_string();
                if trimmed.is_empty() {
                    continue;
                }

                // Handle /clear confirmation inline on the reedline thread
                if trimmed == "/clear" || trimmed == "/new" {
                    println!(
                        "This will clear the current conversation and delete all session memory."
                    );
                    println!("Core memories (long-term facts/preferences) will be preserved.");

                    let confirm_prompt = DefaultPrompt::new(
                        DefaultPromptSegment::Basic("Continue? [y/N] ".to_string()),
                        DefaultPromptSegment::Empty,
                    );
                    match editor.read_line(&confirm_prompt) {
                        Ok(Signal::Success(answer)) => {
                            if !matches!(
                                answer.trim().to_lowercase().as_str(),
                                "y" | "yes"
                            ) {
                                println!("Cancelled.\n");
                                continue;
                            }
                        }
                        _ => {
                            println!("Cancelled.\n");
                            continue;
                        }
                    }
                }

                if tx.send(ReplEvent::Line(trimmed)).is_err() {
                    break;
                }
            }
            Ok(Signal::CtrlC) | Ok(Signal::CtrlD) => {
                break;
            }
            Err(e) => {
                eprintln!("\nError reading input: {e}\n");
                break;
            }
        }
    }
    // Channel drop signals the async side that we're done.
}
