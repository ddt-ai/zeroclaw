use reedline::{
    default_emacs_keybindings, ColumnarMenu, Completer, DefaultPrompt, DefaultPromptSegment,
    EditCommand, Emacs, FileBackedHistory, KeyCode, KeyModifiers, MenuBuilder, Reedline,
    ReedlineEvent, ReedlineMenu, Signal, Span, Suggestion,
};
use std::path::PathBuf;
use tokio::sync::mpsc;

/// Events sent from the reedline thread to the async agent loop.
pub enum ReplEvent {
    /// A line of user input, plus a channel to send output back.
    /// The reedline thread blocks until the sender is dropped.
    Line(String, std::sync::mpsc::Sender<String>),
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
    /// Returns a receiver for user input events. Each event includes a
    /// response channel — the reedline thread blocks until it is dropped,
    /// printing every message it receives before returning to the prompt.
    pub fn spawn(history_path: PathBuf) -> mpsc::UnboundedReceiver<ReplEvent> {
        let (tx, rx) = mpsc::unbounded_channel();

        std::thread::spawn(move || {
            run_reedline(tx, history_path);
        });

        rx
    }
}

fn run_reedline(tx: mpsc::UnboundedSender<ReplEvent>, history_path: PathBuf) {
    let history = Box::new(
        FileBackedHistory::with_file(1000, history_path).expect("failed to open history file"),
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
        .with_edit_mode(edit_mode);

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

                // Create a response channel — we block here until the async
                // side is done processing (drops the sender).
                let (resp_tx, resp_rx) = std::sync::mpsc::channel();

                if tx.send(ReplEvent::Line(trimmed, resp_tx)).is_err() {
                    break;
                }

                // Print all output for this turn, then loop back to read_line
                while let Ok(msg) = resp_rx.recv() {
                    print!("{msg}");
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
