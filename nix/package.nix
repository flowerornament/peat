{ pkgs, src, version }:
let
  # ese's build.rs downloads its embedding model into $CARGO_TARGET_DIR/
  # ese-cache/ unless the files already exist. The nix sandbox has no
  # network, so the two files arrive as fixed-output derivations and are
  # pre-seeded — which also pins the model in the supply chain.
  eseModel = pkgs.fetchurl {
    url = "https://huggingface.co/sentence-transformers/static-retrieval-mrl-en-v1/resolve/main/0_StaticEmbedding/model.safetensors";
    hash = "sha256-Fk/GPun5JnvnN4/L19+Z0JeIovRSRMkqqZrlpXSSVxY=";
  };
  eseTokenizer = pkgs.fetchurl {
    url = "https://huggingface.co/sentence-transformers/static-retrieval-mrl-en-v1/resolve/main/0_StaticEmbedding/tokenizer.json";
    hash = "sha256-0kGmDV6PBMwbKz6e96SSGye/Um2fYFCrkPkmeh+eXGY=";
  };
in
pkgs.rustPlatform.buildRustPackage {
  pname = "peat";
  inherit version src;

  cargoLock = {
    lockFile = src + /Cargo.lock;
    # fold/ese/anny come from the bogkit fork, pinned by rev in Cargo.toml.
    # Bumping the pin means updating this hash (nix build reports the new one).
    outputHashes = {
      "anny-0.0.1" = "sha256-/rMSQuuDrBqe9xov3AWqXiKjCQ8JcsHGDSxq8w8ye2o=";
      "ese-0.1.0" = "sha256-/rMSQuuDrBqe9xov3AWqXiKjCQ8JcsHGDSxq8w8ye2o=";
      "fold-0.0.1" = "sha256-/rMSQuuDrBqe9xov3AWqXiKjCQ8JcsHGDSxq8w8ye2o=";
    };
  };

  preBuild = ''
    export CARGO_TARGET_DIR="$PWD/target"
    mkdir -p "$CARGO_TARGET_DIR/ese-cache"
    cp ${eseModel} "$CARGO_TARGET_DIR/ese-cache/model.safetensors"
    cp ${eseTokenizer} "$CARGO_TARGET_DIR/ese-cache/tokenizer.json"
  '';

  # the oracle twin is red-capable by design: `--ignored` must FAIL, so the
  # default filter is exactly right; keep checks to the green suite
  checkFlags = [ ];

  meta = {
    description = "Agent memory as a fold: an append-forever session ledger with incrementally materialized recall";
    homepage = "https://github.com/flowerornament/peat";
    license = pkgs.lib.licenses.mit;
    mainProgram = "peat";
  };
}
