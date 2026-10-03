use super::*;

/// Distinctive enough that a substring hit is a real leak.
const PLANTED: &str = "NOT-A-REAL-PASSWORD-planted-for-tests";

fn smtp() -> SmtpCredentials {
    SmtpCredentials {
        host: "smtp.example.com".into(),
        port: 587,
        security: SmtpSecurity::Starttls,
        username: "user".into(),
        password: MailSecret::new(PLANTED),
        from_name: "Acme".into(),
        from_email: "hi@acme.test".into(),
    }
}

fn imap() -> ImapCredentials {
    ImapCredentials {
        host: "imap.example.com".into(),
        port: 993,
        username: "user".into(),
        password: MailSecret::new(PLANTED),
    }
}

#[test]
fn credentials_are_tagged_by_provider_on_the_wire() {
    let json = serde_json::to_value(MailCredentials::Smtp(smtp())).unwrap();
    // The tag is what lets a stored blob name its own transport.
    assert_eq!(json["provider"], "smtp");
    assert_eq!(json["host"], "smtp.example.com");
    assert_eq!(json["security"], "starttls");
}

#[test]
fn a_password_reads_in_plain_but_never_writes_out() {
    let raw = serde_json::json!({
        "provider": "smtp",
        "host": "h",
        "port": 25,
        "username": "u",
        "password": PLANTED,
        "from_email": "a@b.test"
    });
    let creds: MailCredentials = serde_json::from_value(raw).unwrap();
    let MailCredentials::Smtp(inner) = &creds;
    assert_eq!(inner.password.expose(), PLANTED);
    assert_eq!(inner.security, SmtpSecurity::Starttls, "defaulted");
    assert_eq!(creds.provider(), MailProvider::Smtp);
    assert_eq!(creds.from_email(), "a@b.test");
    assert_eq!(creds.from_name(), "");

    for rendering in [
        serde_json::to_string(&creds).unwrap(),
        serde_json::to_string(&imap()).unwrap(),
        format!("{creds:?}"),
        format!("{:#?}", smtp()),
        format!("{:?}", imap()),
    ] {
        assert!(!rendering.contains(PLANTED), "leaked: {rendering}");
    }
}

#[test]
fn debug_still_identifies_the_credentials() {
    let rendered = format!("{:?}", MailCredentials::Smtp(smtp()));
    assert!(rendered.contains("Smtp"), "{rendered}");
    assert!(rendered.contains("hi@acme.test"), "{rendered}");
}

#[test]
fn providers_parse_by_name_and_display_lowercase() {
    assert_eq!(
        " SMTP ".parse::<MailProvider>().unwrap(),
        MailProvider::Smtp
    );
    assert_eq!(MailProvider::Smtp.to_string(), "smtp");
    let error = "ses".parse::<MailProvider>().unwrap_err();
    assert!(error.to_string().contains("ses"), "{error}");
}

#[test]
fn security_spells_lowercase_and_defaults_to_starttls() {
    assert_eq!(SmtpSecurity::default(), SmtpSecurity::Starttls);
    assert_eq!(serde_json::to_value(SmtpSecurity::Ssl).unwrap(), "ssl");
    assert_eq!(
        serde_json::from_value::<SmtpSecurity>("none".into()).unwrap(),
        SmtpSecurity::None
    );
}
