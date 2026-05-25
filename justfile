_default:
    just --list

fmt:
    cargo fmt
    cd frontend && oxfmt

lint:
    cargo clippy
    cd frontend && oxlint
    cd frontend && pnpm run lint

ai-debt:
    rg -F 'TODO(ai-review)' -g '!justfile' -g '!CLAUDE.md' -g '!docs/plan.md'
