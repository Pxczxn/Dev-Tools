use std::env;
use std::fs;
use std::io::Cursor;

use minisign::{KeyPair, SecretKeyBox};

fn main() {
    let mut args = env::args().skip(1);
    match args.next().as_deref() {
        Some("--generate") => {
            let path = args.next().expect("private key path");
            let KeyPair { pk, sk } =
                KeyPair::generate_encrypted_keypair(Some(String::new())).expect("generate key");
            if let Some(parent) = std::path::Path::new(&path).parent() {
                let _ = fs::create_dir_all(parent);
            }
            fs::write(&path, sk.to_box(None).expect("box").to_string()).expect("write key");
            println!("{}", pk.to_base64());
        }
        Some("--file") => {
            let file = args.next().expect("file");
            let key = env::var("TAURI_SIGNING_PRIVATE_KEY").expect("TAURI_SIGNING_PRIVATE_KEY");
            let secret = SecretKeyBox::from_string(&key)
                .expect("read key")
                .into_secret_key(Some(String::new()))
                .expect("decrypt key");
            let bytes = fs::read(&file).expect("read file");
            let signature = minisign::sign(None, &secret, Cursor::new(bytes), None, None).expect("sign");
            fs::write(format!("{file}.sig"), signature.to_string()).expect("write signature");
        }
        _ => panic!("usage: sign-update --generate <key-path> | --file <path>"),
    }
}
