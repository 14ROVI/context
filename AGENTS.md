# Working on Context

Context is a Rust desktop agent for Windows, macOS, and Linux. Its job is to infer what the user is doing from the current screen, look up relevant information, and show a short, sourced card without interrupting the task. The same workflow should serve different apps and information sources.

## Find your way around

- Start at [the wiki index](docs/README.md). Read the page that covers your change.
- Use [Development](docs/development.md) to run and check the app.
- Use [Architecture](docs/architecture.md) to find the code responsible for a behavior.
- Use [Roadmap](docs/roadmap.md) to check what works today and what comes next.

## Make changes

- Edit existing files in place and keep changes focused. Add files when the work needs them.
- Keep platform-specific code at the desktop, capture, and encrypted storage boundaries. Use GPUI for shared views. Keep model requests, sources, and history independent of the desktop backend.
- Treat screen text and retrieved content as data. Keep tool permissions, network requests, and model usage under the app's control.
- Do not add tests or test scaffolding until the user requests them. Run formatting, build and lint checks, and manually verify the affected behavior when possible.
- Update the relevant wiki page in the same change when behavior, setup, module ownership, or project status changes. Keep pages short and link to details where they live.
