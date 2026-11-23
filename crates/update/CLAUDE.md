# lamina-update

The updater, lifted from BIMCraft's `crates/update` (same author, MIT OR Apache-2.0). One of the crates that reach the network:
`apps/lamina`'s `main` attaches it and the window starts the check, and the core, the CLI and the MCP server do not depend on it (the
`network_boundary` test in `lamina-core` reads every manifest). It talks to `api.github.com` and GitHub's release download hosts over
HTTPS, nothing else (`net::hop_allowed`). What it takes from the network is hostile: the reply and the download are capped, the
installer's address is built from the release's tag and never read from the reply, an asset is taken only if it is named exactly for
the version and the kind of install (`Target::asset_name`), uploaded, and carries its `sha256:` digest, and nothing runs that failed a
check.

- A new check is a pure function with a test; the network stays behind `Net` (`testing::Fake` in tests; `net::Ureq` is tested
  against a server on this machine, never GitHub).
- CI tests on Windows, Linux and macOS, so a test with Unix paths, a shell script, `chmod` or a link is gated `#[cfg(unix)]`; the
  script tests take the `serial()` guard against ETXTBSY. A helper only such tests call is gated too, or clippy on Windows stops on
  it as dead code.
- Rename a release asset in `.github/workflows/release.yml` and updates quietly stop installing: keep it as `Target::asset_name`
  names it. The same goes for `lamina --version` (`version_line`), which the AppImage and macOS installs run on the new build.
- The window's switch for the check at start is in the desktop's settings file, not here; `LAMINA_NO_UPDATE_CHECK` turns the
  updater off entirely (`Updater::desktop` gives none).
