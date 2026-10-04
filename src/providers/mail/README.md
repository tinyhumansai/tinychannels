# providers::mail

Async, per-credential mail transport. `EmailChannel::send` goes through it, and
hosts that hold many mailboxes (OpenCompany: one per company) use it directly.
Ported from OpenCompany's `server/ops/{mailer,smtp,imap}.rs`.

## Surface

| Item | Feature | Role |
| --- | --- | --- |
| `MailCredentials` (`Smtp(SmtpCredentials)`), `MailProvider`, `SmtpSecurity` | always | Outbound account, tagged `provider` on the wire. |
| `ImapCredentials` | always | Inbound mailbox login. |
| `MailSecret` | always | Password: plaintext in, `"[redacted]"` out via `Debug` and `Serialize`. |
| `OutboundEmail`, `InboundEmail`, `FetchedEmail` | always | What crosses the seam. |
| `MailSender`, `MailReceiver`, `MailError` | always | The seams and their error. |
| `LettreMailSender` | `email-send` | Async `lettre` SMTP. `None`/`Starttls`/`Ssl`; no `AUTH` for an empty username; 30s end-to-end bound (`with_timeout`). |
| `AsyncImapReceiver`, `parse_message` | `email` | Implicit-TLS IMAP (Mozilla roots, `ring`); `INBOX` by default (`with_mailbox`); 30s bound (`with_timeout`). |

The vocabulary links nothing, so a host can accept a `dyn MailSender` and write
test doubles without enabling either feature.

## Receive contract

`fetch_new` runs `UID SEARCH UNSEEN` then `UID FETCH <uids> (UID BODY.PEEK[])`.
`PEEK` leaves `\Seen` unset, so a message the caller fails to file is fetched
again rather than lost. `mark_seen` (`UID STORE +FLAGS.SILENT (\Seen)`) is the
acknowledgement: call it only for UIDs that have been durably filed. An empty
UID list never dials.

## Files

| File | Role |
| --- | --- |
| `mod.rs` | Wiring and feature gates. |
| `types.rs` | Vocabulary, `MailError`, the two traits. |
| `smtp.rs` | `LettreMailSender`. |
| `imap.rs` | `AsyncImapReceiver`, `parse_message`. |
| `*_tests.rs` | Unit tests; SMTP and IMAP run against scripted local fakes. |
