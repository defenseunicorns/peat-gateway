# Project Instructions

## PR Workflow

After creating a PR, self-review the full diff against the originating issue/plan. Post findings as a PR comment covering:
- Correctness gaps (missing error handling, edge cases, panics)
- Security concerns (secrets in memory, missing zeroization, input validation)
- Missing test coverage
- Deviations from the issue requirements

Fix actionable findings before telling the user the PR is ready. The comment serves as a review record.
