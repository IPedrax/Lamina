# lamina-assistant

The AI chat of the desktop: a conversation with a model (Anthropic, OpenAI or Google, the user's choice and the user's key, or a
model on the user's own computer) that
manages the scene on the plate through the commands, and with the printer connection (`crates/printer`) the only code in Lamina that
reaches the network. The desktop attaches
`net::Ureq` and draws the chat; `lamina-core`, the CLI and the MCP server must not depend on this crate. Ported from BIMCraft's
assistant crate (same author, MIT OR Apache-2.0); the safety rules below came with it.

What comes from the network is hostile, and what goes out is the user's:

- A reply is read up to `MAX_REPLY` and parsed leniently; one that is not what the provider documents is an `Err` for the chat to show,
  never a panic. A tool call's arguments are checked before anything runs (`tools::run`).
- Requests go only to the three hosts (`net::allowed`), over HTTPS, or to a local model's server over plain HTTP at 127.0.0.1 or
  ::1 (`net::loopback`: by address, never a name, any port, no credentials), with no redirect followed; the key travels in a header
  and is in no address, no message and no `Debug` (`Request`'s prints no headers). A key or model id with odd characters never
  reaches a header or an address (`key_ok`, `model_ok`, `model_ok_for`).
- `local.rs`: `Provider::Local`, an OpenAI-compatible server on this computer (Ollama, LM Studio), spoken to in Chat Completions,
  which all of them serve. `local::endpoint` takes the address the user typed, refuses any host but this computer, and writes
  `localhost` as 127.0.0.1. No key is needed (`Provider::needs_key`); one is sent only when given. A tool message takes words, so
  pictures follow in a user message, as for OpenAI and Gemini.
- What goes out is capped: a tool result (`MAX_RESULT`), the conversation (`MAX_HISTORY`), the replies one message may take
  (`MAX_ROUNDS`) and the calls honoured in one reply (`MAX_CALLS`).
- The tools run on the window's session through `bridge::run`, as a connected agent's do, and refuse every command whose catalog
  entry has `files != Files::None`: an object's name must not steer the model into the user's disk. The refusal reads the catalog,
  not a list of names, so a command another crate registers (the slicer's `slice`) is covered by what it declares. The model has no
  file tool; the user opens, saves and exports from the app. The commands in `bridge::REPLACES` (`new`, `open`) are refused too:
  they would replace the user's plate and its undo history.
- The saved key (`seal.rs`) has two levels. `lm1`, the default, is sealed with AES-256-GCM under a key made from `LAMINA_KEY_SECRET`, which
  a release build gets from a CI secret (a public development secret otherwise): it is no secret from anyone who has the program, and the
  docs and the page must say so. `lm2`, which the user may add, is under a passphrase alone (600,000 rounds of PBKDF2), so it opens in any
  build; the two bind different associated data, so neither is taken for the other. A change to a format keeps `well_formed` strict (the
  file is hostile) and its known-answer test, whose expected text is made with Python's `hashlib.pbkdf2_hmac` and `cryptography`'s
  `AESGCM`, never with this code.
- A provider is a pure pair: `request(model, key, history) -> net::Request` and `parse(status, body) -> Result<Reply, String>`. A new
  provider or a changed request shape is a test on a recorded reply first (`tests.rs`: text, one call, two calls, malformed, refusal).
  The conversation (`Msg`) is in no provider's form; the model's own turn goes back whole as `Reply::raw`, because thinking blocks,
  reasoning items and thought signatures are valid only in the conversation that made them. So a chat that holds a model turn is not
  continued on another provider (`Chat::send` says so; the user clears it).
- The turn runs in `Chat::poll`, once a frame: the request is on a thread, its reply is taken in the frame, and the tools run on the
  session there. A whole turn is one undo step: each reply's calls run in `Session::transaction` under a label unique to the turn, and
  `merge_steps` joins the turn's steps after each reply (a step the user makes in between keeps them apart). The network stays behind
  `Net` (`testing::Fake`), so no test reaches a provider.
- `attach.rs`: the files sent with a message (a part's photo, a text note). A picked file is copied into a `TempDir` that is the chat's
  (gone on `clear`, `reset` and drop), read back as text (UTF-8, `MAX_FILE_TEXT`) or a picture (`MAX_FILE_IMAGE`, base64 by
  `attach::base64`), and goes out through `user_parts`, which each provider writes in its own form. A file's name is cleaned (`clean`)
  because it is inside `<file name="…">`. File bytes are capped apart from the words (`MAX_FILES_TOTAL`, `Msg::file_bytes`).
- `Reply` carries `thinking` (the provider's summary, shown apart from the answer; `raw` is still what goes back) and `usage` (`Usage`,
  what the provider counted: the chat hands it over with `take_usage`). A new provider reads both.
- `skills.rs` and `skills/*.md`: notes the model reads on demand with `read_skill` (no name lists them), compiled in with
  `include_str!`, so nothing is read from disk at run time. The model sees only names and descriptions until it asks for one. A skill is
  written for Lamina's commands, not borrowed: tests check its header, its size, and that every `command {params}` it shows exists in
  the real catalog (the slicer's included, as a dev dependency). Add a skill by adding a file and a line in `FILES`.
- `todo_write` (`tools::TODO`) is answered by `tools::run` but its list is kept by the chat (`Chat::todos`), not logged as a call.
- `preview` (`tools::PREVIEW`, `tools::picture`) draws the plate with `lamina_core::render` (`PICTURE` pixels) and the chat puts
  the PNG in `Done::image`. Anthropic takes it inside the `tool_result`; OpenAI and Gemini take words only in a function's answer,
  so their clients add a user message (or turn) with the pictures after the answers (`PICTURES`). Pictures count with the files a
  conversation may carry (`Msg::file_bytes`), as every request sends them again.
- Look at `tools::all()`'s schemas when adding a tool: plain types only (Gemini takes a subset of OpenAPI; `gemini::schema` writes a
  free-form object as text that the tool reads back). A test checks it.
- Not here, on purpose: saved chats (roadmap 9.5, now that projects exist) and a chat shared between windows. Add them with their own tests.
- `cargo test -p lamina-assistant` is slow in a debug build unless the root `Cargo.toml` builds `ring` optimised
  (`[profile.dev.package.ring] opt-level = 3`): the passphrase tests make 600,000 rounds of PBKDF2 each.
