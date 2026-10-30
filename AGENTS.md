## ponytail

Use the installed Ponytail skill for all code implementation, refactoring, and review tasks.

Rules:

- Before writing code, follow Ponytail's decision ladder:
  1. Does this need to exist? If not, skip it.
  2. Already exists in the codebase? Reuse it.
  3. Standard library supports it? Use that.
  4. Native platform functionality? Prefer it.
  5. Installed dependency supports it? Reuse it.
  6. Can it be done in one line? Prefer the simple solution.
  7. Otherwise implement the minimum necessary.
- Use Graphify to check for existing implementations before adding new code.
- Prefer small, focused diffs over broad rewrites.
- Avoid unnecessary dependencies, abstractions, wrappers, and boilerplate.
- Never sacrifice correctness, security, validation, accessibility, or necessary tests for fewer lines.
- Apply these rules to every subagent that writes or modifies code.

## caveman

Use the installed Caveman skill for concise technical communication.

Rules:

- Keep responses short, direct, and information-dense.
- Avoid filler, repetition, lengthy introductions, and unnecessary summaries.
- Prefer concise explanations and actionable answers.
- Preserve exact code, commands, paths, identifiers, and error messages.
- Never omit critical warnings, limitations, or necessary technical details.
- Do not shorten code merely to shorten the response.
- Apply the same communication rules to subagents.

## workflow

For codebase tasks, follow this order:

1. Graphify: Locate and understand relevant existing code.
2. Ponytail: Choose the smallest correct solution.
3. Implement and validate the change.
4. Graphify: Run `graphify update .` after code modifications.
5. Caveman: Report changes and validation results concisely.

Graphify discovery rules take precedence over Ponytail's
code-inspection workflow. Caveman must not alter code behavior
or suppress important findings.

## graphify

This project has a knowledge graph at graphify-out/ with god nodes, community structure, and cross-file relationships.

When the user types `/graphify`, use the installed graphify skill or instructions before doing anything else.

Rules:

- When graphify-out/graph.json exists, run `graphify query "<question>"` before your first grep, search or source-file read for a codebase question, and put this rule in every subagent prompt that explores code. Read raw files only for lines the graph or an earlier read already located. Use `graphify path "<A>" "<B>"` for relationships and `graphify explain "<concept>"` for focused concepts. These return a scoped subgraph, usually much smaller than GRAPH_REPORT.md or raw grep output.
- Dirty graphify-out/ files are expected after hooks or incremental updates; dirty graph files are not a reason to skip graphify. Only skip graphify if the task is about stale or incorrect graph output, or the user explicitly says not to use it.
- If graphify-out/wiki/index.md exists, use it for broad navigation instead of raw source browsing.
- Read graphify-out/GRAPH_REPORT.md only for broad architecture review or when query/path/explain do not surface enough context.
- After modifying code, run `graphify update .` to keep the graph current (AST-only, no API cost).
- A committed graphify-out/graph.json is rewritten whole on every build, so a large diff to it in a pull request is expected. Do not flag it as churn or suggest skipping the graph rebuild or its hooks.
