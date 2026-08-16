#!/usr/bin/env -S cargo +nightly -Zscript
---
[package]
edition = "2024"
rust-version = "1.95" # nixpkgs stable

[dependencies]
lazy-regex = "3"
serde = { version = "1", features = ["derive"] }
thiserror = "2"
tokio = { version = "1", features = ["process"] }
toml = "1"
wheel = { git = "https://github.com/fenhl/wheel" }
---

use {
    std::{
        collections::HashMap,
        process::Stdio,
    },
    lazy_regex::{
        bytes_regex_replace_all,
        regex_is_match,
    },
    serde::Deserialize,
    tokio::process::Command,
    wheel::traits::{
        AsyncCommandOutputExt as _,
        IoResultExt as _,
    },
};

#[derive(Deserialize)]
struct CargoToml {
    dependencies: HashMap<String, DependencySpec>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum DependencySpec {
    VersionOnly(String),
    Table {
        version: Option<String>,
    },
}

#[derive(Debug, thiserror::Error)]
enum Error {
    #[error(transparent)] Toml(#[from] toml::de::Error),
    #[error(transparent)] Wheel(#[from] wheel::Error),
    #[error("version {version:?} for dependency {name:?} is not in its shortest form")]
    OverspecifiedVersion {
        name: String,
        version: String,
    },
    #[error(r#"update assets/schema.sql (ssh midos.house 'sudo -u mido pg_dump --schema-only midos_house | sed -e "s/\\\\restrict [[:alnum:]]*/\\\\restrict NSkHPci93sAFqHtSzSNGsBd7dCxhH7NpHe4WhC8jFzIipftC7A6hpgap0hCfbqM/g" | sed -e "s/\\\\unrestrict [[:alnum:]]*/\\\\unrestrict NSkHPci93sAFqHtSzSNGsBd7dCxhH7NpHe4WhC8jFzIipftC7A6hpgap0hCfbqM/g"' > assets/schema.sql)"#)]
    UpdateSchema,
    #[cfg_attr(windows, error("update .sqlx (wsl -d ubuntu-m2 sh -c 'rsync --delete -av /mnt/f/git-stages/github.com/midoshouse/midos.house/ /home/fenhl/wslgit/github.com/midoshouse/midos.house/ --exclude target && env -C /home/fenhl/wslgit/github.com/midoshouse/midos.house /home/fenhl/.cargo/bin/cargo sqlx prepare && rsync --delete -av /home/fenhl/wslgit/github.com/midoshouse/midos.house/.sqlx/ /mnt/f/git-stages/github.com/midoshouse/midos.house/.sqlx/')"))]
    #[cfg_attr(not(windows), error("update .sqlx (cargo sqlx prepare)"))]
    UpdateSqlx,
}

impl wheel::CustomExit for Error {
    fn exit(self, cmd_name: &'static str) {
        match self {
            Self::Wheel(wheel::Error::CommandExit { name, output }) => {
                eprintln!("{cmd_name}: command `{name}` exited with {}", output.status);
                eprintln!("stdout:");
                eprintln!("{}", String::from_utf8_lossy(&output.stdout));
                eprintln!("stderr:");
                eprintln!("{}", String::from_utf8_lossy(&output.stderr));
            }
            _ => {
                eprintln!("{cmd_name}: {self}");
                eprintln!("debug info: {self:?}");
            }
        }
        std::process::exit(1)
    }
}

#[wheel::main(custom_exit)]
async fn main() -> Result<(), Error> {
    let manifest = toml::from_slice::<CargoToml>(&Command::new("git").arg("show").arg(":Cargo.toml").stdout(Stdio::piped()).check("git show").await?.stdout)?;
    for (name, value) in manifest.dependencies {
        if let Some(version) = match value {
            DependencySpec::VersionOnly(version) => Some(version),
            DependencySpec::Table { version } => version,
        } && !regex_is_match!(r"^(?:0\.)*[1-9][0-9]*$", &version) {
            return Err(Error::OverspecifiedVersion { name, version })
        }
    }

    cfg_select! {
        windows => {
            println!("cargo deny");
            Command::new("cargo").arg("+stable").arg("deny").arg("check").arg("advisories").arg("bans").check("cargo deny").await?;

            println!("cargo test");
            Command::new("cargo").arg("+stable").arg("test").spawn().at_command("cargo test")?.check("cargo test").await?;

            println!("cargo msrv");
            Command::new("cargo").arg("+stable").arg("msrv").arg("verify").spawn().at_command("cargo msrv")?.check("cargo msrv").await?;

            println!("wsl apt-get");
            Command::new("wsl").arg("-d").arg("ubuntu-m2").arg("sudo").arg("-n").arg("apt-get").arg("install").arg("-y").arg("pkg-config").arg("libssl-dev").spawn().at_command("wsl apt-get")?.check("wsl apt-get").await?;

            println!("wsl rustup");
            Command::new("wsl").arg("-d").arg("ubuntu-m2").arg("/home/fenhl/.cargo/bin/rustup").arg("update").arg("stable").spawn().at_command("wsl rustup")?.check("wsl rustup").await?;

            println!("wsl cargo install");
            Command::new("wsl").arg("-d").arg("ubuntu-m2").arg("/home/fenhl/.cargo/bin/cargo").arg("install").arg("sqlx-cli").spawn().at_command("wsl cargo install")?.check("wsl cargo install").await?;

            println!("wsl rsync");
            Command::new("wsl").arg("-d").arg("ubuntu-m2").arg("rsync").arg("--mkpath").arg("--delete").arg("-av").arg("/mnt/f/git-stages/github.com/midoshouse/midos.house/").arg("/home/fenhl/wslgit/github.com/midoshouse/midos.house/").arg("--exclude").arg("target").spawn().at_command("wsl rsync")?.check("wsl rsync").await?; // copy the tree to the WSL file system to improve compile times

            println!("wsl cargo check");
            Command::new("wsl").arg("-d").arg("ubuntu-m2").arg("env").arg("-C").arg("/home/fenhl/wslgit/github.com/midoshouse/midos.house").arg("/home/fenhl/.cargo/bin/cargo").arg("check").spawn().at_command("wsl cargo check")?.check("wsl cargo check").await?;

            println!("wsl cargo sqlx");
            if Command::new("wsl").arg("-d").arg("ubuntu-m2").arg("env").arg("-C").arg("/home/fenhl/wslgit/github.com/midoshouse/midos.house").arg("/home/fenhl/.cargo/bin/cargo").arg("sqlx").arg("prepare").arg("--check").spawn().at_command("wsl cargo sqlx")?.check("wsl cargo sqlx").await.is_err() {
                return Err(Error::UpdateSqlx)
            }
        }
        _ => {
            println!("cargo deny");
            Command::new("cargo").arg("deny").arg("check").arg("advisories").arg("bans").check("cargo deny").await?;

            println!("cargo test");
            Command::new("cargo").arg("test").spawn().at_command("cargo test")?.check("cargo test").await?;

            println!("cargo msrv");
            Command::new("cargo").arg("msrv").arg("verify").spawn().at_command("cargo msrv")?.check("cargo msrv").await?;

            println!("cargo sqlx");
            if Command::new("cargo").arg("sqlx").arg("prepare").arg("--check").spawn().at_command("cargo sqlx")?.check("cargo sqlx").await.is_err() {
                return Err(Error::UpdateSqlx)
            }
        }
    }

    let prepared_schema = Command::new("git").arg("show").arg(":assets/schema.sql").stdout(Stdio::piped()).check("git show").await?.stdout;
    let prepared_schema = bytes_regex_replace_all!(r"\\(un)?restrict \w*", &prepared_schema, |_, un| {
        let mut buf = vec![b'\\'];
        buf.extend_from_slice(un);
        buf.extend_from_slice(b"restrict NSkHPci93sAFqHtSzSNGsBd7dCxhH7NpHe4WhC8jFzIipftC7A6hpgap0hCfbqM");
        buf
    });
    let production_schema = Command::new("ssh").arg("midos.house").arg("sudo -u mido pg_dump --schema-only midos_house").stdout(Stdio::piped()).check("ssh midos.house pg_dump").await?.stdout;
    let production_schema = bytes_regex_replace_all!(r"\\(un)?restrict \w*", &production_schema, |_, un| {
        let mut buf = vec![b'\\'];
        buf.extend_from_slice(un);
        buf.extend_from_slice(b"restrict NSkHPci93sAFqHtSzSNGsBd7dCxhH7NpHe4WhC8jFzIipftC7A6hpgap0hCfbqM");
        buf
    });
    if prepared_schema != production_schema {
        return Err(Error::UpdateSchema)
    }

    Ok(())
}
