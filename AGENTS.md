# Repository guidance

## Product

- Captures is a work-in-progress, cross-platform screen capture utility. Keep public wording broad enough to cover screenshots, GIFs, and video.
- Clearly separate current features from roadmap ideas. Do not present tentative work as shipped or promise release dates unless the repository already commits to them.
- Treat automatic per-merge builds as experimental Nightlies. Do not publish a stable release until the maintainer explicitly starts that channel.

## Repository map

- `apps/desktop` contains the Tauri desktop application and its React UI.
- `apps/web` contains the static project website.
- `crates` contains the shared Rust capture and platform-integration crates.
- `DEVELOPMENT.md` contains local setup, validation, and packaging instructions.
- `docs/releases.md` contains maintainer release and recovery procedures.
- `apps/web/vite.config.ts` fetches homepage history from the GitHub API at build time. Do not check in hardcoded or generated commit history.

## Working conventions

- Keep changes focused on the request and preserve existing behavior unless a change is intentional.
- Do not overwrite, stage, or publish unrelated work already present in the checkout.
- Reuse established patterns in the repository before introducing a new abstraction or dependency.
- Keep this file concise and update it when a recurring repository convention or correction should persist across future work.

## Visual design

- Build from neutral foundations: off-white or white and charcoal or near-black surfaces, high-contrast typography, subtle borders, and restrained shadows and corner radii.
- Do not force one accent color across the product. Use a small multi-accent palette selectively and consistently for actions, selection and focus, status, diagrams, and occasional feature moments while keeping most interface chrome neutral.
- Establish hierarchy with typography, spacing, and modular layout before color. Prefer generous whitespace, clear grids or cards, and concise copy. Keep default page and application chrome neutral; reserve gradients or large color fields for contained feature moments.
- Product imagery and illustrations may carry more color than their surrounding interface. Preserve accessible contrast and give recurring accent colors stable meanings.

## Documentation

- Every pull request must leave the root `README.md` accurate. Update it when a change affects features, platform support, shortcuts, setup, build commands, privacy, networking, releases, or deployment.
- Keep the root README product-focused. Put local setup, validation, and packaging details in `DEVELOPMENT.md`.
- If a pull request does not need a README edit, still verify that its changes do not make the README inaccurate; do not add no-op wording solely to touch the file.
- Keep the README concise and user-oriented. Put detailed maintainer release procedures in `docs/releases.md` and web-specific operations in `apps/web/README.md`.
- Keep current behavior and roadmap sections distinct, especially for features that are not implemented yet.

## Validation

- Run `npm run check` for the default repository gate. It includes release-version tests, desktop typechecking/lint/tests, and the production web build.
- For Rust changes, also run `cargo fmt --all -- --check`, `cargo test --workspace`, and `cargo clippy --workspace --all-targets -- -D warnings`.
- For Docker changes, build the image when Docker is available. If it is unavailable, validate the affected build stages directly and report that limitation.
- Report exactly which checks ran and any checks that could not run.

## Pull requests

- Prefer a focused pull request over pushing directly to `main` unless explicitly asked otherwise.
- Use a concise title and description covering what changed, why it changed, user or developer impact, and validation.
- Open pull requests as drafts unless the user asks for a ready review.
