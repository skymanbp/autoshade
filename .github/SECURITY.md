# Security policy

## Supported version

The **latest tagged release** only. Fixes land on `main` and ship in the next
release; older tags are not patched in place.

## Reporting a vulnerability

Use GitHub's private vulnerability reporting: the repository's **Security** tab
→ **Report a vulnerability**. That opens a private advisory visible only to you
and the maintainer, so a working exploit can be discussed before there is a fix
to point people at.

No email address is published for this, on purpose — a private advisory is
tracked, attributable and has a disclosure timeline attached; an inbox has none
of those.

Please include what you would put in a bug report: version, front-end, and the
exact steps. A proof of concept is welcome and never required.

## Scope

By default, Autoshop keeps the source library read-only. If the configured Delivery folder is inside or above a photo’s folder, that delivery subtree is intentionally writable; Settings warns when this removes the folder’s protection. “Export .xmp beside the photo” is the separate, confirmed per-photo sidecar exception.

The parts with a real attacker are the ones that take input from somewhere else: the
**local web server** (`serve`), the **settings and key-resolution rules**, the
**child processes** (the `claude` verifier and the Python sidecars), and the
**third-party decoders** that parse untrusted RAW files. Both threat models are
already written down — the server's in
[README § Privacy, trust, and paid-feature boundary](../README.md#privacy-trust-and-paid-feature-boundary) and
[ARCHITECTURE § 4.9 "What the local web server refuses"](../docs/ARCHITECTURE.md#49-what-the-local-web-server-refuses),
the settings-trust boundary in the same README section. If you have found a way
past what those documents claim, that is exactly the report worth making — and
so is a case where the *documentation* overstates the guarantee.

Out of scope: anything that requires an attacker to already be running code as
you, and the behaviour of the AI endpoints you configure — those are third-party
services under your own account.
