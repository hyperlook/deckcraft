# DeckCraft — Working Notes

Refer to [AGENTS.md](AGENTS.md) for core architecture and stability guidelines.

## Development Workflow
1. Verify workspace status with `cargo test` or `cargo clippy`.
2. Inspect or update relevant milestone documents (e.g., `ROADMAP.md`).

## Core Engineering Habits
- **Command Pattern**: Every user-visible action is implemented as an engine command with unit tests (`crates/engine/src/cmd/*`).
- **UI Verification**: Validate UI changes using `deckcraft --sample --control 7990` + `ui.screenshot`, or render via `deckcraft-cli render`.
- **Defensive Code**: Zero unhandled unwraps/panics in production code; handle errors cleanly with `Result` and `?`.
- **Keep Documentation Synchronized**: Update `ROADMAP.md` when completing major capabilities.
