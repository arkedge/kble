fn main() {
    println!("cargo:rerun-if-changed=Cargo.toml");

    notalawyer_build::build();
}
