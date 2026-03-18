Implement feature: $ARGUMENTS

## Prerequisites
- Plan must exist at `plans/$ARGUMENTS-plan.md`
- Read it first

## Steps

For each task in the plan:

1. **Implement** the task following SPEC.md contracts
2. **Verify** after each task:
   - Rust: `cargo clippy -- -W clippy::all && cargo test`
   - Frontend: `npm run build && npx prettier --check src/`
   - Integration: `cargo tauri dev` (if applicable)
3. **Commit** with descriptive message after each verified task
4. **Report** progress to user

## Validation Gates

After completing all tasks:
1. Run the code-reviewer agent on all changed files
2. Check acceptance criteria from the plan
3. Present results to user — WAIT for approval before marking complete

## If something goes wrong
- Do NOT brute-force. Stop and reassess.
- If the plan needs changing, update `plans/$ARGUMENTS-plan.md` and inform the user.
