// Copyright (C) Microsoft Corporation. All rights reserved.

//! Crypto types defined in `BiosInterface.h`

use self::packed_nums::*;
use crate::hyperv::common::EfiStatus64NoErrorBit;
use open_enum::open_enum;
use zerocopy::AsBytes;
use zerocopy::FromBytes;
use zerocopy::FromZeroes;

#[allow(non_camel_case_types)]
mod packed_nums {
    pub type u64_ne = zerocopy::U64<zerocopy::NativeEndian>;
}

open_enum! {
    /// Command types for CRYPTO_COMMAND_DESCRIPTOR.
    ///
    /// These correlate with the semantics of the UEFI runtime variable services.
    /// Note that all commands other than GET_RANDOM_NUMBER have been deprecated.
    ///
    /// MsvmPkg: `CRYPTO_COMMAND`
    #[derive(AsBytes, FromBytes, FromZeroes)]
    pub enum CryptoCommand: u32 {
        COMPUTE_HASH = 0,
        VERIFY_RSA_PKCS_1 = 1,
        VERIFY_PKCS_7 = 2,
        VERIFY_AUTHENTICODE = 3,
        LOG_EVENT_DEPRECATED = 4,
        GET_RANDOM_NUMBER = 5,
    }
}

/// MsvmPkg: `CRYPTO_COMMAND_DESCRIPTOR`
#[repr(C)]
#[derive(Debug, Clone, Copy, AsBytes, FromBytes, FromZeroes)]
pub struct CryptoCommandDescriptor {
    pub command: CryptoCommand,
    pub status: EfiStatus64NoErrorBit,
}

/// MsvmPkg: `CRYPTO_COMMAND_DESCRIPTOR`
#[repr(C)]
#[derive(Debug, AsBytes, FromBytes, FromZeroes)]
pub struct CryptoGetRandomNumberParams {
    pub buffer_address: u64_ne,
    pub buffer_size: u32,
}