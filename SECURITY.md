# Security Policy

TinyChannels is a channel and messaging library for OpenHuman harness
communication. Security-sensitive areas include message routing, transport
adapters, channel identity, harness boundaries, event metadata, prompt or
context payloads, and credentials passed through runtime context.

Report concerns to `contact@tinyhumans.ai`.

## Supported Versions

TinyChannels is pre-1.0. Security fixes target the `main` branch until the
project starts maintaining release branches.

## Reporting A Vulnerability

Please do not open a public issue for a suspected vulnerability.

Report security concerns by emailing `security@tinyhumans.ai` with:

- a description of the issue
- affected versions or commits
- reproduction steps or proof of concept
- impact assessment
- any suggested fix or mitigation

We will acknowledge reports as quickly as practical and coordinate disclosure
before publishing details.

## Scope

Examples of in-scope issues:

- unintended cross-channel message leakage
- unsafe handling of credentials or secrets
- harness communication bypassing expected policy boundaries
- incorrect channel identity or routing metadata
- dependency vulnerabilities with a practical exploit path in TinyChannels

Examples generally out of scope:

- low-quality model output by itself
- unsafe workflows caused by downstream applications granting broad authority
- vulnerabilities in downstream applications that use TinyChannels incorrectly
