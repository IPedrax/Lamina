# Questions for the morning

Decisions the overnight run took on its own, and what blocked it. Each says the question, the options, the default taken and why.

## Pushing was refused

Every write to GitHub from the cloud session was refused with 403 ("Claude doesn't have GitHub access to IPedrax/Lamina for your
organization"): `git push` to `main`, to `roadmap-overnight` and to the session's own branch, and the GitHub API (creating a branch).
Reads work.

- Options: stop; keep working and lose everything if the session ends; keep working, commit locally, and keep a copy elsewhere.
- Taken: commit locally on `main`, one commit per item as IPedrax, and keep the whole series as a patch file on a private claude.ai
  page (the run's report gives its link), updated as items finish. Nothing is lost if the container is reclaimed.
- To finish: connect GitHub for Claude (claude.ai settings, connectors) or install the Claude GitHub App on the repository, then
  push from the session; or apply the patch file on `main` at `6e4a60b` with `git am` and push.

## Projects: `open` and `new` start a new undo history

- Options: make them undoable steps; or start a new history, as BIMCraft does.
- Taken: a new history. The print settings live beside the scene (`Session::print`), not in it, so undoing an `open` would bring back
  the old plate with the new project's settings. The window asks before New or Open loses unsaved changes, and the assistant never
  runs either.

## Projects: what counts as an unsaved change

- Taken: a changed plate always; changed print settings only when the project has a file or the plate is not empty. An empty,
  unnamed window with other settings loses nothing, since the settings file keeps them.
