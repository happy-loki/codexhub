# Contributing

Thanks for considering contributing to `codex-remote-gateway`.

This project sits between Codex App remote-control and Feishu IM, so small protocol changes can affect message routing and approval behavior. Please keep changes conservative and easy to review.

## Development Setup

```powershell
cargo fmt
cargo test
cargo build
```

For approval-related changes:

```powershell
cargo test approval
```

## Design Rules

- Keep Codex as the source of truth.
- Do not change Codex cwd, model, sandbox, approval policy, or environment from the bridge.
- Prefer forwarding official app-server protocol payloads instead of inventing bridge-specific semantics.
- Keep Codex App launch clean: configuration should point it at the local backend, but `codex-remote-gateway` should not wrap or launch Codex.
- Feishu UI should be compact and stateful; avoid repeated explanatory messages when card state can show the result.

## Pull Request Checklist

- Explain the user-facing behavior change.
- Include verification steps.
- Add or update docs for command/config/protocol changes.
- Avoid committing local config, credentials, state files, logs, or build outputs.

## Security

Do not include real Feishu app credentials, user open ids, private chat ids, screenshots with sensitive content, or local project paths in issues or pull requests.
