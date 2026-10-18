# The updater flip (prepared, not applied)

The update check and the install path are built and tested, and they are compiled out of every shipped
build by the `live-updates` cargo feature (off by default in `crates/flare-update/Cargo.toml` and
`crates/flare/Cargo.toml`). This file is the patch to switch them on. It is written down now so that the
day the repository goes public is a short, checked change and not a rediscovery. Nothing here is applied
yet. Do not apply it before the repository is public.

## Steps, in order

1. **Make the repository public first.** The release workflow refuses the feature while the repository is
   not public (Task 28: the Build step reads `gh repo view --json visibility` and throws). Check it:

   ```
   gh repo view Maxaubert/Flare --json visibility
   ```

   It must print `"PUBLIC"`. Only then set the repository variable that the release workflow reads:

   ```
   gh variable set FLARE_LIVE_UPDATES --body on --repo Maxaubert/Flare
   ```

2. **Make the feature a default in both crates.** In `crates/flare-update/Cargo.toml` and
   `crates/flare/Cargo.toml` change the `[features]` line `default = []` to:

   ```
   default = ["live-updates"]
   ```

   (`crates/flare/Cargo.toml` already maps `live-updates = ["flare-update/live-updates"]`.)

3. **Replace the PRIVACY.md sentence.** In `PRIVACY.md`, replace the paragraph that begins "Flare makes
   **no request on its own** in this version." (and its "switched off" sentence) with the following. The
   bullet "**Installing an update:** not available yet." becomes "**Installing an update:** clicking Install
   downloads that release's installer from GitHub." (it stays in the "only when you ask" list).

   > Flare makes **one request on its own**:
   >
   > - **The update check.** The installed app asks GitHub for the latest release of this repository
   >   (`api.github.com/repos/Maxaubert/Flare/releases`) when it starts and every four hours after. The
   >   request carries only what any HTTPS request carries (your IP address, and a user agent naming the
   >   app); GitHub's own privacy statement covers what GitHub does with it. Development and test builds
   >   never make it.
   >
   > Everything else happens **only when you ask for it**:

   The rest of the list (installing an update, dictation, links, Sonara, what you run) stays as it is.

4. **Turn the two "off" tests into their "on" twins.** Run the workspace tests once with the feature
   on by default, then make these edits so the suite asserts the new state:

   - `crates/flare/tests/privacy.rs`, `the_update_check_sentence_says_off`: rename it to
     `the_update_check_sentence_says_on` and change the asserted text from `"switched off"` to
     `"The update check."`.
   - `a_default_build_has_no_live_mode` (the test that asserts the default build has no live update
     source; it lives beside the `live-updates` gate in `flare-update`): change its assertion from "the
     feature is absent" to "the feature is present", and rename it `a_default_build_has_live_mode`.

   Then run the whole workspace suite:

   ```
   cargo nextest run --workspace
   ```

   (Add the build folder the orchestrator names with `--target-dir` where the builders run it.)

   Every test passes, including the inventory scan (`every_network_capable_file_is_inventoried`): the
   update check's network lines in `crates/flare-update/src/velopack_feed.rs` already have their row, so
   no inventory change is needed for the flip itself.

5. **Release `0.x.y`.** Bump the version in the root `Cargo.toml` (`[workspace.package] version`), merge
   through a pull request, and let the release workflow build the feature build (`FLARE_LIVE_UPDATES=on`)
   and publish `v<version>`. Confirm on the release page that `releases.win.json` and the Setup file are
   attached.

## What the flip changes, in one list

- `Cargo.toml` features: two lines (step 2).
- `PRIVACY.md`: the update-check paragraph (step 3), in the same pull request as the flip.
- Tests: the two "off" assertions become their "on" twins (step 4).
- Repository: the `FLARE_LIVE_UPDATES` variable (step 1). No secret changes.

## Checks before the flip is merged

- `gh repo view --json visibility` prints `PUBLIC`.
- `PRIVACY.md` names the GitHub API request, and says it runs at start and every four hours.
- The release workflow build step passes with the variable set, and fails with it set on a private
  repository (the guard in `release_files.rs`).
