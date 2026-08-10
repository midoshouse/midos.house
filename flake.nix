{
    inputs.flake.url = "github:fenhl/flake";
    outputs = attrs: attrs.flake.lib {
        devShells = {
            default = { pkgs, ... }: {
                packages = with pkgs; [
                    cargo
                ];
            };
            pre-commit = { pkgs, ... }: {
                packages = with pkgs; [
                    cargo
                    cargo-msrv
                    clang
                    python3
                ];
            };
        };
    };
}
