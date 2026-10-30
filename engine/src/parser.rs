//! The shift-reduce driver, recorded one move at a time.
//!
//! Each [`StackEntry`] bundles `{state, symbol}`, so a reduce pops |α| entries
//! (the textbook's alternating stack pops 2 × |α| items).

use crate::error::{EngineError, Result};
use crate::grammar::{count_input, tokenize_input, Grammar, END};
use crate::items::production_display;
use crate::limits;
use crate::table::Table;
use crate::trace::{Action, Recorder, StackEntry, Step};

/// Check a test string against the grammar's terminals and split it into
/// symbols, using the same tokenization mode the grammar was read in.
pub fn tokenize_test_string(grammar: &Grammar, text: &str) -> Result<Vec<String>> {
    // Counted before splitting, so an over-long string costs one pass.
    let count = count_input(text, grammar.mode);
    if count > limits::MAX_TEST_SYMBOLS {
        return Err(EngineError::new(format!(
            "The test string is {} symbols long, over the {} symbol limit.",
            count,
            limits::MAX_TEST_SYMBOLS
        ))
        .hint("The parse trace shows the whole stack at every step and is meant to be read one step at a time. Try a shorter string that exercises the same productions."));
    }
    let tokens = tokenize_input(text, grammar.mode);
    // Otherwise an unknown symbol gives a bare REJECT that blames the parser.
    for token in &tokens {
        if !grammar.is_known_terminal(token) {
            let terminals = grammar.terminals.join(" ");
            let hint = if grammar.is_nonterminal(token) {
                format!("`{token}` is a non-terminal in this grammar. A test string is made of terminals only - the things the parser actually reads.")
            } else {
                format!(
                    "The grammar is being read in {} mode. Check that the string is written the same way.",
                    grammar.mode.label()
                )
            };
            return Err(EngineError::new(format!(
                "`{token}` is not a terminal in this grammar. Terminals are: {terminals}."
            ))
            .hint(hint));
        }
    }
    Ok(tokens)
}

/// Run the parse, recording every move. Returns whether the string was accepted.
pub fn run(
    grammar: &Grammar,
    table: &Table,
    tokens: &[String],
    rec: &mut Recorder,
) -> Result<bool> {
    let mut input: Vec<String> = tokens.to_vec();
    input.push(END.to_string());

    let mut stack: Vec<StackEntry> = vec![StackEntry { state: 0, symbol: None }];
    let mut pos: usize = 0;

    rec.push(Step::ParseInit {
        stack: stack.clone(),
        input: input.clone(),
        explain: format!(
            "The parse begins in state I0 with an empty symbol stack, and the end marker $ appended to the input. At every step the parser looks at the state on top of the stack and the next input symbol, then does whatever ACTION says: shift, reduce, accept, or - if the cell is empty - reject. The input to parse is {}.",
            input.join(" ")
        ),
    })?;

    // Each turn records exactly one step or returns, so this caps the phase.
    for _ in 1..limits::MAX_PARSE_STEPS {
        // Every step shows the whole stack, so trace size grows quadratically.
        if stack.len() > limits::MAX_PARSE_STACK {
            return Err(stack_too_deep(table));
        }
        let state = stack.last().map(|e| e.state).unwrap_or(0);
        let lookahead = input.get(pos).cloned().unwrap_or_else(|| END.to_string());
        let actions = table.actions_at(state, &lookahead);

        let Some(action) = actions.first().cloned() else {
            let expected = table.expected_at(grammar, state);
            rec.push(Step::ParseReject {
                input_pos: pos,
                state,
                lookahead: lookahead.clone(),
                expected: expected.clone(),
                explain: reject_explain(state, pos, &lookahead, &expected),
            })?;
            return Ok(false);
        };

        // A conflicted cell: take the first entry, and say the trace is meaningless.
        let conflict_note = if actions.len() > 1 {
            let shown: Vec<String> = actions.iter().map(Action::short).collect();
            format!(
                " This cell is in conflict ({}); the first action was taken arbitrarily, so nothing after this point is a meaningful parse.",
                shown.join(" / ")
            )
        } else {
            String::new()
        };

        match action {
            Action::Shift { state: to } => {
                stack.push(StackEntry { state: to, symbol: Some(lookahead.clone()) });
                pos += 1;
                let remaining = input.len().saturating_sub(pos);
                rec.push(Step::ParseShift {
                    symbol: lookahead.clone(),
                    to_state: to,
                    stack: stack.clone(),
                    input_pos: pos,
                    explain: format!(
                        "ACTION[{state}, {}] is shift {to}, so `{}` is read from the input and pushed onto the stack together with state I{to}. The stack is now {}, and {} symbol{} of input remain{}.{conflict_note}",
                        display_symbol(&lookahead),
                        lookahead,
                        render_stack(&stack),
                        remaining,
                        if remaining == 1 { "" } else { "s" },
                        if remaining == 1 { "s" } else { "" },
                    ),
                })?;
            }

            Action::Reduce { production } => {
                let p = grammar.production(production)?;
                let count = p.rhs.len();
                if stack.len() < count + 1 {
                    return Err(EngineError::new(format!(
                        "internal error: reduce by production {production} needs {count} stack entries but only {} are present",
                        stack.len().saturating_sub(1)
                    )));
                }
                stack.truncate(stack.len().saturating_sub(count));
                let exposed = stack.last().map(|e| e.state).unwrap_or(0);
                let Some(goto) = table.goto_at(exposed, &p.lhs) else {
                    let expected = table.expected_at(grammar, exposed);
                    rec.push(Step::ParseReject {
                        input_pos: pos,
                        state: exposed,
                        lookahead: lookahead.clone(),
                        expected: expected.clone(),
                        explain: format!(
                            "After reducing by production {production} ({}) the exposed state is I{exposed}, but GOTO[{exposed}, {}] is empty, so the parser has nowhere to go. This can only happen when a conflict earlier in the trace was resolved arbitrarily. REJECT.",
                            production_display(grammar, production)?,
                            p.lhs
                        ),
                    })?;
                    return Ok(false);
                };
                stack.push(StackEntry { state: goto, symbol: Some(p.lhs.clone()) });
                rec.push(Step::ParseReduce {
                    production,
                    popped: count,
                    exposed_state: exposed,
                    goto_state: goto,
                    stack: stack.clone(),
                    input_pos: pos,
                    explain: format!(
                        "ACTION[{state}, {}] is reduce {production} ({}). {} That exposes state I{exposed}, and GOTO[{exposed}, {}] = {goto}, so {} is pushed with state I{goto}. The stack is now {}. The input pointer has not moved: a reduction rewrites the stack, it never consumes input.{conflict_note}",
                        display_symbol(&lookahead),
                        production_display(grammar, production)?,
                        pop_phrase(count, &p.rhs.join(" ")),
                        p.lhs,
                        p.lhs,
                        render_stack(&stack),
                    ),
                })?;
            }

            Action::Accept => {
                rec.push(Step::ParseAccept {
                    explain: format!(
                        "ACTION[{state}, $] is accept. The whole input has been read and the stack has been reduced to the start symbol, so the string is in the language described by this grammar. ACCEPT.{conflict_note}"
                    ),
                })?;
                return Ok(true);
            }

            Action::Goto { .. } => {
                return Err(EngineError::new(
                    "internal error: a GOTO entry appeared in the ACTION table",
                ));
            }
        }
    }

    // With no conflicts the parse was simply long; with conflicts it may be looping.
    let hint = if table.is_valid() {
        "This table has no conflicts, so the parser was not going round in circles: the parse is simply long. Every symbol read is one shift, and a grammar with chains of unit productions (A → B) or with ε-productions then spends several more steps reducing before it reads the next. Try a shorter test string."
    } else {
        "This table has conflicts, and in a conflicted cell the parser takes the first action it finds. That choice can send it round a loop of reductions that never reads another symbol - check the Parsing Table tab for the conflicted cells. A long test string over a grammar with chains of unit productions or ε-productions can also run out of steps with nothing wrong, so try a shorter one as well."
    };
    Err(EngineError::new(format!(
        "The string parse did not finish within {} steps.",
        limits::MAX_PARSE_STEPS
    ))
    .hint(hint))
}

/// The refusal for a stack past [`limits::MAX_PARSE_STACK`].
fn stack_too_deep(table: &Table) -> EngineError {
    let hint = if table.is_valid() {
        "This table has no conflicts, so the parser was not looping: this grammar really does hold that many symbols on the stack for this string. That is more than twice the longest test string allowed, so most of them were put there by ε-productions and stand for nothing read. Try a shorter test string."
    } else {
        "This table has conflicts, and in a conflicted cell the parser takes the first action it finds. Here that choice has it reducing by an ε-production over and over, pushing a symbol each time and never reading another - check the Parsing Table tab for the conflicted cells."
    };
    EngineError::new(format!(
        "The parse stack grew past {} entries.",
        limits::MAX_PARSE_STACK
    ))
    .hint(hint)
}

// --- text helpers ----------------------------------------------------------

fn display_symbol(symbol: &str) -> String {
    if symbol == END {
        "$".to_string()
    } else {
        format!("`{symbol}`")
    }
}

fn render_stack(stack: &[StackEntry]) -> String {
    let parts: Vec<String> = stack
        .iter()
        .map(|e| match &e.symbol {
            Some(s) => format!("{s} {}", e.state),
            None => format!("{}", e.state),
        })
        .collect();
    parts.join(" ")
}

fn pop_phrase(count: usize, rhs: &str) -> String {
    match count {
        0 => "The right-hand side is empty, so nothing is popped.".to_string(),
        1 => format!("The right-hand side has one symbol, so one entry ({rhs}) is popped off the stack."),
        n => format!("The right-hand side has {n} symbols, so {n} entries ({rhs}) are popped off the stack."),
    }
}

fn reject_explain(state: usize, pos: usize, lookahead: &str, expected: &[String]) -> String {
    let seen = display_symbol(lookahead);
    let where_ = if lookahead == END {
        "at the end of the input".to_string()
    } else {
        format!("at position {} of the input", pos + 1)
    };
    if expected.is_empty() {
        return format!(
            "ACTION[{state}, {seen}] is empty, and state I{state} has no entries at all in its ACTION row, so the parser is stuck {where_}. REJECT."
        );
    }
    let list: Vec<String> = expected
        .iter()
        .map(|s| if s == END { "$ (end of input)".to_string() } else { s.clone() })
        .collect();
    format!(
        "ACTION[{state}, {seen}] is empty: in state I{state} the parser can only continue on {}. It found {seen} {where_} instead, so there is no legal move and the string is not in the language described by this grammar. REJECT.",
        list.join(", ")
    )
}
