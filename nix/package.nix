{
  lib,
  rustPlatform,
  makeWrapper,
  git,
}:

let
  cargoToml = lib.importTOML ../Cargo.toml;
in
rustPlatform.buildRustPackage {
  pname = cargoToml.package.name;
  version = cargoToml.package.version;

  # Only the files that affect the cargo build, so editing the README,
  # flake, or docs does not trigger a rebuild.
  src = lib.fileset.toSource {
    root = ./..;
    fileset = lib.fileset.unions [
      ../Cargo.toml
      ../Cargo.lock
      ../src
      # Embedded at compile time via include_str! in main.rs and views.rs.
      ../logo.txt
    ];
  };

  # Vendor straight from the committed lockfile — no vendor hash to maintain.
  cargoLock.lockFile = ../Cargo.lock;

  nativeBuildInputs = [ makeWrapper ];

  # The test suite shells out to a real `git`, which needs a binary on PATH and
  # a configured identity + writable HOME inside the sandbox.
  nativeCheckInputs = [ git ];
  preCheck = ''
    export HOME=$(mktemp -d)
    git config --global user.email "build@lazygitrs.nix"
    git config --global user.name "lazygitrs build"
    git config --global init.defaultBranch main
    # A merge test asserts two separate conflict blocks, which only holds under
    # a diff3-style conflict marker layout (the default "merge" style coalesces
    # adjacent conflicts into one).
    git config --global merge.conflictStyle diff3
  '';

  # lazygitrs shells out to the `git` binary at runtime; guarantee it is found.
  postInstall = ''
    wrapProgram $out/bin/lazygitrs \
      --prefix PATH : ${lib.makeBinPath [ git ]}
  '';

  meta = {
    description = cargoToml.package.description;
    homepage = "https://github.com/blankeos/lazygitrs";
    license = lib.licenses.mit;
    mainProgram = "lazygitrs";
  };
}
