//! Ling procedural macros

use proc_macro::TokenStream;
use syn::{parse_macro_input, DeriveInput};

#[proc_macro_derive(LingType)]
pub fn derive_ling_type(input: TokenStream) -> TokenStream {
    let _input = parse_macro_input!(input as DeriveInput);
    // Generate Ling type metadata
    TokenStream::new()
}
