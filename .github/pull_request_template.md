<!--

Thanks for taking the time to contribute to Ekko!

We are always excited about pull requests!

If the pull request fixes any open issues, reference the corresponding
issues, e.g: `Fixes #321`.

A couple of things worth knowing before you start:

- `cargo test` includes golden tests that diff Ekko's terminal output,
  byte for byte, against output captured from the original taskbook JS
  build. If you are changing anything in `src/render.rs`, expect those to
  be the tests that catch you -- and see `tests/golden/README.md` before
  you consider updating a `.ans` reference file.
- CI runs `cargo clippy --all-targets -- -D warnings`, so lints have to be
  clean, not merely non-fatal.

Including test results, screenshots/gifs if applicable/possible, alongside
new features and bug fixes is something that we strongly encourage.

Thank you so much for all of the time and effort you put in the project!

-->
