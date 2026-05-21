# Support

ForkTTY is a community project maintained on a best-effort basis. Please
use the channel that best matches what you need.

## I think I found a bug

Open a [bug report](https://github.com/Lucenx9/forktty/issues/new?template=bug_report.yml).

Before filing, please:

- Confirm you are on a [supported platform](docs/QA.md#supported-platforms)
  and a current ForkTTY release (`forktty --version`).
- Run `forktty doctor` and paste the output.
- Include reproduction steps, expected vs. actual behavior, and any
  relevant log output from `~/.local/share/forktty/logs/`.

## I have a feature idea

Open a [feature request](https://github.com/Lucenx9/forktty/issues/new?template=feature_request.yml).
Please describe the user-facing outcome you want and the workflow it
supports. Scope alignment lives in [`ROADMAP.md`](ROADMAP.md).

## I have a question

Use [GitHub Discussions](https://github.com/Lucenx9/forktty/discussions)
for usage questions, configuration help, and general topics. The issue
tracker is reserved for actionable bugs and feature work.

## I have a security report

Please **do not** open a public issue.

Follow the disclosure process in [`SECURITY.md`](SECURITY.md), or use
[GitHub private vulnerability reporting](https://github.com/Lucenx9/forktty/security/advisories/new)
directly.

## Supported platforms

The current supported runtime baseline is libadwaita 1.4+ and VTE 0.76+,
which matches Ubuntu 24.04 LTS and newer distro packages. See
[`docs/QA.md`](docs/QA.md) for the full coverage grid and the per-distro
expectations.

## Response expectations

- Security reports: acknowledged within 48 hours where feasible.
- Bug reports: triaged on a rolling basis; no SLA during alpha.
- Feature requests: evaluated against the [roadmap](ROADMAP.md) and may
  be deferred or declined to keep scope manageable.
