# Repository guidance

## Product

- Captures is a work-in-progress, cross-platform screen capture utility. Keep public wording broad enough to cover screenshots, GIFs, and video.
- Clearly separate current features from roadmap ideas. Do not present tentative work as shipped or promise release dates unless the repository already commits to them.
- Treat dated Preview builds as experimental. Do not publish a stable release until the maintainer explicitly starts that channel.
- Public downloads and updates are pinned to the approved Tauri tag in `scripts/preview-release.mjs`. Native builds are opt-in until the maintainer accepts parity; passing CI does not authorize promotion. Match Tauri behavior and visuals, without adding unrequested controls such as Share.

## Repository map

- `apps/desktop` contains the Tauri desktop application and its React UI.
- `apps/web` contains the TanStack website and a Node server for its same-origin `/api/*` routes. AWS k3s runs that process; Cloudflare proxies `captur.es` and caches hashed static assets.
- `crates` contains the shared Rust capture and platform-integration crates.
- `DEVELOPMENT.md` contains local setup, validation, and packaging instructions.
- `docs/releases.md` contains maintainer release and recovery procedures.
- `apps/web/vite.config.ts` fetches homepage history from the GitHub API at build time and excludes Dependabot commits from Latest changes. Do not check in hardcoded or generated commit history.

## Working conventions

- Keep changes focused on the request and preserve existing behavior unless a change is intentional.
- Do not overwrite, stage, or publish unrelated work already present in the checkout.
- Assume other agents may be working in this repository concurrently. Do new work in a dedicated Git worktree, and never stash, switch, or overwrite another agent's checkout.
- Reuse established patterns in the repository before introducing a new abstraction or dependency.
- Treat macOS, Windows, and Linux parity as the default. Prefer shared UI and logic that works on every supported platform; when a fix or feature must be platform-specific, implement or stub the equivalent path on the others (or explicitly gate with `cfg` / runtime checks), and document any unavoidable limitation in the commit message and README if user-facing. Do not assume “works on my Mac” is enough—call out what was and was not verified on other platforms.
- Deliver the native rewrite in cross-platform feature slices, not a complete macOS app followed by ports. Follow `docs/native-rewrite.md`; record macOS, Windows, X11, and Wayland implementation/verification status in each slice's commit messages. Stubs and unverified hosts do not complete a parity gate.
- Keep this file concise and update it when a recurring repository convention or correction should persist across future work.

## Visual design

- The desktop UI is built on tokens. `shared/design.css` holds the neutral ramp, semantic surfaces, spacing, radii, elevation, and motion; `shared/themes.css` holds accent and signal palettes. Style against those tokens instead of literal colors or pixel values.
- `apps/desktop/ui/src/styles.css` is only an import list. Real rules live in `apps/desktop/ui/src/styles/`: `base`, `primitives`, then one module per family of surfaces.
- Regular windows follow the light/dark/system appearance setting. Surfaces that float over the user's desktop (capture overlay, capture menu, recording controls, mini previews, saved/hidden recording notices, the post-update / launch tooltip) always use the fixed `--glass-*` media palette so they stay legible on any wallpaper. The update notice is a solid `--surface-raised` card in a transparent native window. The launch notice is a dark glass pill with a CSS triangle caret pointing at the tray or menu bar icon — not a rotated square. Without reliable icon geometry, use a compact, left-aligned rounded rectangle without a caret; do not stretch it to fill the native window.
- Reserve the accent for the primary capture action, selection, and focus. Keep status colors stable: signal for recording and destructive, green for saved, blue for progress.
- Review UI changes with the dev harness (`?view=…&mock=1`, see DEVELOPMENT.md) before claiming a surface looks right.
- Native product controls should use `captures_app::icons` shipping SVG paths on every OS (AppKit through `ShippingIcons`), not OS-specific symbols or separately drawn approximations.
- Build from neutral foundations: off-white or white and charcoal or near-black surfaces, high-contrast typography, subtle borders, and restrained shadows and corner radii.
- Do not force one accent color across the product. Use a small multi-accent palette selectively and consistently for actions, selection and focus, status, diagrams, and occasional feature moments while keeping most interface chrome neutral.
- Establish hierarchy with typography, spacing, and modular layout before color. Prefer generous whitespace, clear grids or cards, and concise copy. Keep default page and application chrome neutral; reserve gradients or large color fields for contained feature moments.
- Product imagery and illustrations may carry more color than their surrounding interface. Preserve accessible contrast and give recurring accent colors stable meanings.

## Documentation

- Every change must leave the root `README.md` accurate. Update it when a change affects features, platform support, shortcuts, setup, build commands, privacy, networking, releases, or deployment.
- Keep the root README product-focused. Put local setup, validation, and packaging details in `DEVELOPMENT.md`.
- If a change does not need a README edit, still verify that it does not make the README inaccurate; do not add no-op wording solely to touch the file.
- Keep the README concise and user-oriented. Put detailed maintainer release procedures in `docs/releases.md` and web-specific operations in `apps/web/README.md`.
- Keep current behavior and roadmap sections distinct, especially for features that are not implemented yet.

## Validation

- Run `npm run check` for the default repository gate. It includes release-version tests, desktop typechecking/lint/tests, and the production web build.
- For Rust changes, also run `cargo fmt --all -- --check`, `cargo test --workspace`, and `cargo clippy --workspace --all-targets -- -D warnings`.
- For performance work, report absolute costs as well as percentages. A small absolute slowdown can be acceptable for a much larger gain elsewhere; disclose the tradeoff for review instead of treating every timing regression as a blocker. Preserve behavior and output.
- For Docker changes, build the image when Docker is available. If it is unavailable, validate the affected build stages directly and report that limitation.
- Report exactly which checks ran and any checks that could not run.

## Shipping

Speed beats ceremony: there are no real users yet, so ship, watch, and fix forward.

- Commit and push straight to `main`; no pull requests. Run the local checks for what you changed first (Validation above), then `git pull --rebase origin main` and push. Never force-push `main` or rewrite other agents' commits. Write a concise commit message covering what changed, why, and what was verified.
- Never put literal CI-skip directives in commit messages, even to explain them: `CI` and the image builds would silently not run, leaving no image to deploy for that SHA.
- Every push to `main` runs `CI` (about 10 minutes: frontend, account API, Rust on macOS/Windows/Linux, native wgpu checks), `AWS image` (web) and `AWS API image` (about 2 minutes each, no path filters). `Native Windows shortcut ownership` runs when `shortcuts.rs` changes. `Native wgpu workbench`, `Native macOS workbench` and CI's unsigned installer builds run nightly and on demand; dispatch them (`gh workflow run <file> --repo joswayski/captures --ref main`) only when a native GUI or platform change needs that check before the nightly run.
- After pushing, deploy what changed yourself, server before web, waiting for each step to succeed before the next. `SHA` is your pushed commit; find its runs with `gh run list --repo joswayski/captures --commit "$SHA"`, wait with `gh run watch <id> --exit-status` (infrastructure runs: `gh run list --repo joswayski/infrastructure --workflow <file> --limit 1`), and report what you deployed.
  1. Infrastructure or secrets the change needs, in `joswayski/infrastructure`, following that repository's guidance.
  2. If the API changed (`apps/api`, the workspace crates it builds, `Cargo.lock`), after `AWS API image` succeeds for `SHA`: `gh workflow run deploy-captures-api.yml --repo joswayski/infrastructure --ref main -f git_sha="$SHA"`. The API applies database migrations on startup.
  3. If the website changed (`apps/web`, `shared`, `docs/images`, root package files), after `AWS image` succeeds for `SHA`: `gh workflow run deploy-captures.yml --repo joswayski/infrastructure --ref main -f git_sha="$SHA"`.
  4. Desktop apps: `release.yml` (Captures Preview) stays manual. Dispatch it, or change the approved public tag in `scripts/preview-release.mjs`, only when the maintainer asks (see `docs/releases.md`).
- If `CI`, an image build or a deploy fails, fix it forward with another commit. Roll back a broken server with the deploy workflow's `rollback_from_sha` input only when a fix will take longer than a few minutes.
- Still ask the owner first for anything irreversible: destructive or data-dropping migrations, deleting infrastructure or data, rotating secrets.
