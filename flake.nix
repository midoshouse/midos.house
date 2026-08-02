{
    inputs = {
        nixpkgs.url = "https://flakehub.com/f/NixOS/nixpkgs/*.tar.gz";
        flake-utils.url = "github:numtide/flake-utils";
    };
    outputs = attrs: attrs.flake-utils.lib.eachDefaultSystem (system: let
        pkgs = import attrs.nixpkgs {
            inherit system;
        };
    in {
        devShells = {
            default = pkgs.mkShell {
                packages = with pkgs; [
                    cargo
                ];
            };
            pre-commit = pkgs.mkShell {
                packages = with pkgs; [
                    cargo
                    cargo-msrv
                    clang
                    python3
                ];
            };
        };
    });
}
