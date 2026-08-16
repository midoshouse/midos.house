{
    inputs = {
        flake.url = "github:fenhl/flake";
        rust-overlay = {
            url = "github:oxalica/rust-overlay";
            inputs.nixpkgs.follows = "flake/nixpkgs";
        };
    };
    outputs = attrs: attrs.flake.lib {
        overlays = [
            attrs.rust-overlay.overlays.default # required for cargo-script
        ];
        devShells = {
            default = { pkgs, ... }: {
                packages = with pkgs; [
                    cargo
                    sqlx-cli
                ];
            };
            pre-commit = { pkgs, ... }: {
                packages = with pkgs; [
                    rust-bin.nightly.latest.default # nightly cargo, required to run the pre-commit script
                    cargo-deny
                    cargo-msrv
                    clang
                    python3
                    sqlx-cli
                ];
            };
        };
    };
}
