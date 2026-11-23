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

## Sending to a printer: upload only unless asked

- Options: `send_to_printer` starts the print by default (`print=true`, as the roadmap's request shape has it); or uploads only
  unless `print: true` is given.
- Taken: upload only by default. Starting a print is a physical act; the window has two buttons, Send to printer and Send and print,
  so the choice is the user's each time.

## Sending to a printer: not for agents

- Options: register `send_to_printer` everywhere; or only where the user acts.
- Taken: the window and `lamina-cli run` have it; the MCP server's session does not, and the assistant refuses it because it reads a
  file. Say if MCP agents should be able to send (perhaps behind a flag such as `--allow-printer`).

## The printer's API key: no passphrase yet

- Taken: Remember seals the key under the program's secret (`lm1`), as the assistant's default is, and the panel says it is
  scrambling, not protection. The assistant's passphrase option (`lm2`) is not offered for the printer key yet.

## Cancel during an upload

- Since 7.2 the slice and the upload run on a thread. Cancel stops the slice at once, but an upload already under way is not
  interrupted (ureq has no way to abort a request from another thread): the window stops waiting and says it was cancelled, and the
  file may still reach the printer. Taken because the upload is short and a half-sent file is worse than a whole one. Say if a
  cancelled upload should be followed by a delete on the printer.

## Releases need secrets, and were not run (roadmap 7.7)

- The release workflow needs the repository secret `LAMINA_KEY_SECRET` (any long random text, kept private): without it the
  release's tests fail on purpose, so no release seals keys under the public development secret. For a macOS build Gatekeeper
  accepts, also `APPLE_CERTIFICATE`, `APPLE_CERTIFICATE_PASSWORD`, `APPLE_ID`, `APPLE_PASSWORD` and `APPLE_TEAM_ID`; without them
  the DMG is signed ad hoc and Gatekeeper asks before it opens.
- Not run here: GitHub refused every write, so no tag could start the workflow. Only the Linux tarball was built and checked in
  this session; the AppImage step downloads appimagetool, which this session was not allowed to run, and the MSI and DMG need
  Windows and macOS runners. shellcheck passes on the scripts and the workflow parses. The first tag will be their first real run.
- Not lifted from BIMCraft: deb, rpm, Flatpak, Arch and riscv64 packages, the APT repository and the Discord posts. Say which of
  them Lamina should have.

## The licence: MIT kept (roadmap 8.4)

- Question: stay MIT, or move to MIT OR Apache-2.0 as BIMCraft is (Apache-2.0 adds a patent grant and a patent retaliation clause).
- Options: keep MIT; dual-license MIT OR Apache-2.0 (add LICENSE-APACHE, set `license` in the workspace manifest, update NOTICE,
  the README and the installers).
- Taken: MIT kept. Changing a project's licence is the owner's decision, not a night's run, and MIT can still become dual later, as
  the code is the owner's and what is lifted from BIMCraft is MIT OR Apache-2.0 already. The rest of 8.4 is done: every release
  now carries THIRD-PARTY-NOTICES.txt, and a test fails if any dependency's licence offers no permissive choice.

## Braces in start G-code are expressions now (roadmap 5.5)

- Since 5.5, `{...}` in a start or end G-code is worked out as an expression, and `{{` writes a brace. A G-code that had a literal
  brace (a Klipper macro pasted in, say) now fails its check with a message saying where, rather than going out changed. Taken
  because PrusaSlicer's start G-code uses braces the same way, so users expect it. Say if a different escape is wanted.

## Blocked: real test prints (roadmap 0.2)

- Printing the calibration cube, the single-wall box and the bridge test on a Marlin and a Klipper printer needs the printers, and
  measuring them needs calipers. Not possible from a cloud session; nothing here has touched a printer. The golden G-code tests
  (0.1) guard the output meanwhile. `lamina-cli slice` makes the three files when you are ready.

## Not run: the picture of the window

- `picture_of_the_window` (apps/lamina) needs a graphics adapter, which the session lacks, so no new screenshot was made of the
  File menu, the printer connection or the new print settings. `docs/lamina-window.png` is the old one. Installing Mesa's software
  Vulkan driver to get an adapter was not allowed in this session.
- So the user guide (8.3) shows pictures from Lamina's own CPU renderer instead: the plate in the 3D view and the 3D G-code preview
  (`docs/images/`, drawn by `cargo test -p lamina --lib -- --ignored docs_pictures`). A picture of the whole window should be added
  to it from a machine with a graphics card.
