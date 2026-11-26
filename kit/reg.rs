// SPDX-License-Identifier: MIT
//! Registers that are read and written as 8 or 16 bits, where m-bootloader's [`Reg`](crate::hw::Reg) is 32.

/// Where the chip has registers (m-bootloader's docs/SPEC.md §2.2): not its RAM, nor the flash.
const fn is_register(address: u32) -> bool {
    matches!(address, 0x1_0000..0x2_0000 | 0x4_0000..0x6_0000 | 0x1ee_e000..0x1ee_f000 | 0x1f0_0000..0x1f0_c000)
}

macro_rules! narrow {
    ($name:ident, $t:ty) => {
        #[derive(Clone, Copy)]
        pub struct $name(u32);

        impl $name {
            /// The register at `ADDRESS`, which does not compile unless the chip has registers there.
            pub const fn at<const ADDRESS: u32>() -> $name {
                const { assert!(is_register(ADDRESS) && ADDRESS % core::mem::size_of::<$t>() as u32 == 0) };
                $name(ADDRESS)
            }

            #[inline(always)]
            fn pointer(self) -> *mut $t {
                core::ptr::with_exposed_provenance_mut(self.0 as usize)
            }

            #[inline(always)]
            pub fn read(self) -> $t {
                // Safety: a register of the chip, which Rust has no reference to.
                unsafe { self.pointer().read_volatile() }
            }

            #[inline(always)]
            pub fn write(self, value: $t) {
                unsafe { self.pointer().write_volatile(value) }
            }

            #[inline(always)]
            pub fn set(self, bits: $t) {
                self.write(self.read() | bits)
            }

            #[inline(always)]
            pub fn clear(self, bits: $t) {
                self.write(self.read() & !bits)
            }
        }
    };
}

narrow!(Reg8, u8);
narrow!(Reg16, u16);
