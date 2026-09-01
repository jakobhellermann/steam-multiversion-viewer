_default:
    just --list

fmt:
    cargo fmt
    cd frontend && oxfmt

lint:
    cargo clippy
    cd frontend && oxlint
    cd frontend && pnpm run lint

test:
    cargo test
    cd frontend && pnpm run test

build:
	cd frontend && pnpm run build
	cargo build --release

licenses:
	cargo about generate about.hbs -o THIRD-PARTY-NOTICES.html

ai-debt *filter:
    rg -F 'TODO(ai-review)' -g '!justfile' -g '!CLAUDE.md' -g '!docs/plan.md' {{ filter }}
