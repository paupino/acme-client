/// Easy to use Let's Encrypt client to issue and renew TLS certs

extern crate acme_client;
extern crate openssl;
extern crate clap;
extern crate env_logger;
extern crate openssl_sys;


use std::io;
use std::path::Path;
use std::collections::HashSet;
use acme_client::Directory;
use acme_client::error::Result;
use clap::{Arg, App, SubCommand, ArgMatches};


fn main() {
    let matches = App::new(env!("CARGO_PKG_NAME"))
        .version(env!("CARGO_PKG_VERSION"))
        .about(env!("CARGO_PKG_DESCRIPTION"))
        .usage("acme-client sign -D example.org -P /var/www -k domain.key -o domain.crt\
                \n    acme-client revoke -K user_or_domain.key -C signed.crt")
        .subcommand(SubCommand::with_name("sign")
            .about("Signs a certificate")
            .display_order(1)
            .arg(Arg::with_name("USER_KEY_PATH")
                .help("User private key path to use it in account registration.")
                .long("user-key")
                .short("U")
                .takes_value(true))
            .arg(Arg::with_name("DOMAIN_KEY_PATH")
                .help("Domain private key path to use it in CSR generation.")
                .short("K")
                .long("domain-key")
                .takes_value(true))
            .arg(Arg::with_name("DOMAIN")
                .help("Domain name to obtain certificate. You can use more than one domain name.")
                .short("D")
                .long("domain")
                .multiple(true)
                .takes_value(true))
            .arg(Arg::with_name("PUBLIC_DIR")
                .help("Directory to save ACME simple http challenge. This option is required.")
                .short("P")
                .long("public-dir")
                .takes_value(true))
            .arg(Arg::with_name("EMAIL")
                .help("Contact email address (optional).")
                .short("E")
                .long("email")
                .takes_value(true))
            .arg(Arg::with_name("DOMAIN_CSR")
                .help("Path to domain certificate signing request.")
                .short("C")
                .long("domain-csr")
                .takes_value(true))
            .arg(Arg::with_name("SAVE_USER_KEY")
                .help("Path to save private user key.")
                .long("save-user-key")
                .short("u")
                .takes_value(true))
            .arg(Arg::with_name("SAVE_DOMAIN_KEY")
                .help("Path to save domain private key.")
                .short("k")
                .long("save-domain-key")
                .takes_value(true))
            .arg(Arg::with_name("SAVE_DOMAIN_CSR")
                .help("Path to save domain certificate signing request.")
                .long("save-csr")
                .short("S")
                .takes_value(true))
            .arg(Arg::with_name("SAVE_SIGNED_CERTIFICATE")
                .help("Path to save signed certificate. Default is STDOUT.")
                .short("o")
                .long("save-crt")
                .takes_value(true))
            .arg(Arg::with_name("CHAIN")
                .help("Chains the signed certificate with Let's Encrypt Authority X3 \
                       (IdenTrust cross-signed) intermediate certificate.")
                .short("c")
                .long("chain")
                .takes_value(false))
            .arg(Arg::with_name("DNS_CHALLENGE")
                 .help("Use DNS challenge instead of HTTP. This option requires user \
                        to generate a TXT record for domain")
                 .short("d")
                 .long("dns")
                 .takes_value(false)))
        .subcommand(SubCommand::with_name("revoke")
            .about("Revokes a signed certificate")
            .display_order(2)
            .arg(Arg::with_name("USER_KEY")
                .help("User or domain private key path.")
                .long("user-key")
                .short("K")
                .required(true)
                .takes_value(true))
            .arg(Arg::with_name("SIGNED_CRT")
                .help("Path to signed domain certificate to revoke.")
                .long("signed-crt")
                .short("C")
                .required(true)
                .takes_value(true)))
        .arg(Arg::with_name("verbose")
             .help("Show verbose output")
             .short("v")
             .multiple(true))
        .get_matches();

    init_logger(matches.occurrences_of("verbose"));

    let res = if let Some(matches) = matches.subcommand_matches("sign") {
        sign_certificate(matches)
    } else if let Some(matches) = matches.subcommand_matches("revoke") {
        revoke_certificate(matches)
    } else {
        println!("{}", matches.usage());
        Ok(())
    };

    if let Err(e) = res {
        eprintln!("{}", e);
        ::std::process::exit(1);
    }
}



fn sign_certificate(matches: &ArgMatches) -> Result<()> {
    // get domain names from --domain-csr or --domain arguments
    // FIXME: there is so many unncessary string conversations in the following code
    let domains: Vec<String> = if let Some(csr_path) = matches.value_of("DOMAIN_CSR") {
        names_from_csr(csr_path)?.into_iter().collect()
    } else {
        matches.values_of("DOMAIN")
            .ok_or("You need to provide at least one domain name")?.map(|s| s.to_owned()).collect()
    };

    // domains could be empty if provided --csr doesn't contain any
    if domains.is_empty() {
        return Err("You need to provide at least one domain name with --domain argument \
                   or from --csr".into());
    }

    let directory = Directory::lets_encrypt()?;

    let mut account_registration = directory.account_registration();

    if let Some(email) = matches.value_of("EMAIL") {
        account_registration = account_registration.email(email);
    }

    if let Some(user_key_path) = matches.value_of("USER_KEY_PATH") {
        account_registration = account_registration.pkey_from_file(user_key_path)?;
    }

    let account = account_registration.register()?;

    for domain in &domains {
        let authorization = account.authorization(domain)?;
        if !matches.is_present("DNS_CHALLENGE") {
            let challenge = authorization.get_http_challenge().ok_or("HTTP challenge not found")?;
            challenge.save_key_authorization(matches.value_of("PUBLIC_DIR")
                                                 .ok_or("--public-dir not defined. \
                                                            You need to define a public \
                                                            directory to use http challenge \
                                                            verification")?)?;
            challenge.validate()?;
        } else {
            let challenge = authorization.get_dns_challenge().ok_or("DNS challenge not found")?;
            println!("Please create a TXT record for _acme-challenge.{}: {}\n\
                      Press enter to continue",
                     domain,
                     challenge.signature()?);
            io::stdin().read_line(&mut String::new()).unwrap();
            challenge.validate()?;
        }
    }

    let dv: Vec<&str> = domains.iter().map(String::as_str).collect();
    let mut certificate_signer = account.certificate_signer(dv.as_slice());

    if let Some(domain_key_path) = matches.value_of("DOMAIN_KEY_PATH") {
        if let Some(csr_path) = matches.value_of("DOMAIN_CSR") {
            certificate_signer = certificate_signer.csr_from_file(domain_key_path, csr_path)?;
        } else {
            certificate_signer = certificate_signer.pkey_from_file(domain_key_path)?;
        }
    }

    let certificate = certificate_signer.sign_certificate()?;
    let signed_certificate_path = matches.value_of("SAVE_SIGNED_CERTIFICATE")
        .ok_or("You need to save signed certificate")?;
    if matches.is_present("CHAIN") {
        certificate.save_signed_certificate_and_chain(None, signed_certificate_path)?;
    } else {
        certificate.save_signed_certificate(signed_certificate_path)?;
    }

    if let Some(path) = matches.value_of("SAVE_DOMAIN_KEY") {
        certificate.save_private_key(path)?;
    }
    if let Some(path) = matches.value_of("SAVE_DOMAIN_CSR") {
        certificate.save_csr(path)?;
    }
    if let Some(path) = matches.value_of("SAVE_USER_KEY") {
        account.save_private_key(path)?;
    }

    Ok(())
}


fn revoke_certificate(matches: &ArgMatches) -> Result<()> {
    let directory = Directory::lets_encrypt()?;
    let account = directory.account_registration()
        .pkey_from_file(matches.value_of("USER_KEY")
                            .ok_or("You need to provide user \
                                   or domain private key used \
                                   to sign certificate.")?)?
        .register()?;
    account.revoke_certificate_from_file(matches.value_of("SIGNED_CRT")
                                             .ok_or("You need to provide \
                                                    a signed certificate to \
                                                    revoke.")?)?;
    Ok(())
}


fn init_logger(level: u64) {
    let level = match level {
        0 => "",
        1 => "acme_client=info",
        _ => "acme_client=debug",
    };
    let mut builder = env_logger::LogBuilder::new();
    builder.parse(&::std::env::var("RUST_LOG").unwrap_or(level.to_owned()));
    let _ = builder.init();
}


fn names_from_csr<P: AsRef<Path>>(csr_path: P) -> Result<HashSet<String>> {
    use std::fs::File;
    use std::io::Read;
    use std::slice;
    use openssl::x509::{X509Req, X509Extension};
    use openssl::nid;
    use openssl::stack::Stack;
    use openssl::types::OpenSslTypeRef;
    use std::os::raw::{c_int, c_long, c_uchar};
    use openssl::types::OpenSslType;

    fn read_file<P: AsRef<Path>>(path: P) -> Result<Vec<u8>> {
        let mut file = File::open(path)?;
        let mut content = Vec::new();
        file.read_to_end(&mut content)?;
        Ok(content)
    }

    let csr = X509Req::from_pem(&read_file(csr_path)?)?;

    let mut names = HashSet::new();

    // add CN to names first
    if let Some(cn) = csr.subject_name().entries_by_nid(nid::COMMONNAME).nth(0) {
        names.insert(String::from_utf8_lossy(cn.data().as_slice()).into_owned());
    }

    unsafe {
        #[repr(C)]
        struct Asn1StringSt {
            length: c_int,
            type_: c_int,
            data: *mut c_uchar,
            flags: c_long
        }
        extern "C" {
            fn X509_REQ_get_extensions(
                req: *mut openssl_sys::X509_REQ
            ) -> *mut openssl_sys::stack_st_X509_EXTENSION;
            fn X509v3_get_ext_by_NID(
                x: *const openssl_sys::stack_st_X509_EXTENSION,
                nid: c_int,
                lastpos: c_int
            ) -> c_int;
            fn X509_EXTENSION_get_data(
                ne: *mut openssl_sys::X509_EXTENSION,
            ) -> *mut Asn1StringSt;
        }
        let extensions = X509_REQ_get_extensions(csr.as_ptr());
        if !extensions.is_null() {
            let san_extension_idx = X509v3_get_ext_by_NID(extensions,
                                                          openssl_sys::NID_subject_alt_name,
                                                          -1);
            if let Some(san_extension) = Stack::<X509Extension>::from_ptr(extensions)
                .iter().nth(san_extension_idx as usize) {

                let extension_data = X509_EXTENSION_get_data(san_extension.as_ptr());
                let slc = slice::from_raw_parts((*extension_data).data,
                                                (*extension_data).length as usize);
                parse_asn1_octet_str(slc).iter().for_each(|n| {
                    names.insert(n.to_string());
                });
            }
        }
    }

    Ok(names)
}


fn parse_asn1_octet_str(s: &[u8]) -> Vec<String> {
    let mut iter = s.split(|n| *n == 130);
    let mut names = Vec::new();
    if iter.next().is_some() {
        for s in iter {
            let mut v = s.to_vec();
            v.remove(0); // remove first element which is length
            let name = String::from_utf8_lossy(&v);
            names.push(name.into_owned());
        }
    }
    names
}

#[test]
fn test_names_from_csr() {
    let _ = env_logger::init();
    let mut names = HashSet::new();
    names.insert("cn.example.com".to_owned());
    names.insert("www.example.com".to_owned());
    names.insert("example.com".to_owned());
    let names_from_csr = names_from_csr("tests/domain_with_san.csr").unwrap();
    assert_eq!(names, names_from_csr);
}
