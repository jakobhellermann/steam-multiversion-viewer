_default:
    just --list

ai-debt:
    rg -F 'TODO(ai-review)' -g '!justfile' -g '!CLAUDE.md' -g '!docs/plan.md'
