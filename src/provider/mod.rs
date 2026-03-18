// Re-export the Provider trait and types from the smelt-provider crate
pub use smelt_provider::*;

// Small providers that don't have heavy deps stay here
pub mod cloudflare;
pub mod google_workspace;
pub mod mock;

// Re-export provider implementations from workspace crates
pub mod aws {
    pub use smelt_aws::AwsProvider;
    #[allow(unused_imports)]
    pub use smelt_aws::aws::*;
}

pub mod gcp {
    pub use smelt_gcp::GcpProvider;
    #[allow(unused_imports)]
    pub use smelt_gcp::gcp::*;
}
