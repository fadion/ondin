# Ondin — the assistant

> **Status: what is left.** The assistant itself is built and its design is in `design/assistant.md`.
> This file keeps only what is still unbuilt; when a part is built, its rules move there. A `§n`
> cited against this file is a section of `design/assistant.md`, which keeps the numbers.

## `/ai`

Command Mode's `/ai` (`vm.md` §11) is a later door into the same assistant: a request typed in the
bar goes to the configured harness, and what it proposes lands as one undoable step. It waits on
Command Mode's bar.

## Script blocks in replies

A Luau code block in a reply gains *Open in script editor* once the script editor (`vm.md` §10)
exists. Until then it renders as any other code block.

## Open questions

1. **`/ai`'s look in the bar** (`vm.md` §13, question 7).
