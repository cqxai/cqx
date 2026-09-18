//! Seven ways to reach a process, a socket or the environment that a purely
//! syntactic reader has trouble with.

pub mod h1_import_alias;
pub mod h2_type_alias;
pub mod h3_const;
pub mod h4_macro;
pub mod h5_trait_object;
pub mod h6_typed_field;
pub mod h7_one_hop;
pub mod h8_env_provenance;
