-- CosinusOS CryptoFS Layer
-- cryptofs.ads — sector-level encryption/decryption and integrity tagging
-- Implements: XOR-stream cipher (placeholder for AES-256-GCM),
--             per-sector HMAC-like tags, key derivation mixing

with Interfaces;   use Interfaces;
with Interfaces.C; use Interfaces.C;
with System;

package CryptoFS
   with SPARK_Mode => On
is

   SECTOR_SIZE    : constant := 512;
   KEY_SIZE       : constant := 32;
   TAG_SIZE       : constant := 16;
   NONCE_SIZE     : constant := 12;
   MAX_KEY_SLOTS  : constant := 8;
   CRYPTO_MAGIC   : constant Unsigned_32 := 16#C051_4346#;  -- "COSCF"

   -- Cipher IDs — algo field in key slot
   CIPHER_NONE    : constant Unsigned_8 := 0;
   CIPHER_XOR     : constant Unsigned_8 := 1;  -- fast, weak — dev only
   CIPHER_SIPHASH : constant Unsigned_8 := 2;  -- SipHash-2-4 stream
   CIPHER_CHACHA  : constant Unsigned_8 := 3;  -- ChaCha20 (future)

   ERR_OK         : constant int := 0;
   ERR_NO_KEY     : constant int := -1;
   ERR_BAD_TAG    : constant int := -2;
   ERR_CIPHER     : constant int := -3;
   ERR_KEY_FULL   : constant int := -4;
   ERR_INVALID    : constant int := -5;

   subtype LBA_Type  is Unsigned_64;
   type Key_Bytes is array (0 .. KEY_SIZE  - 1) of Unsigned_8;
   type Tag_Bytes is array (0 .. TAG_SIZE  - 1) of Unsigned_8;
   type Nonce_T   is array (0 .. NONCE_SIZE - 1) of Unsigned_8;

   type Sector_Buffer is array (0 .. SECTOR_SIZE - 1) of Unsigned_8;
   type Pad_4_Bytes   is array (0 .. 3) of Unsigned_8;

   -- Per-sector crypto header — prepended to encrypted sectors on disk
   -- (stored out-of-band in practice, here kept simple for kernel use)
   type Sector_Tag is record
      Magic    : Unsigned_32;
      LBA      : LBA_Type;
      Counter  : Unsigned_64;
      Tag      : Tag_Bytes;
      Checksum : Unsigned_32;
   end record
   with Alignment => 8;

   -- Key slot
   type Crypto_Key_Slot is record
      Magic     : Unsigned_32;
      Slot_Id   : Unsigned_8;
      Cipher    : Unsigned_8;
      Flags     : Unsigned_16;  -- bit 0=active, 1=readonly, 2=region_bound
      Key       : Key_Bytes;
      Nonce     : Nonce_T;
      LBA_Bound_Start : LBA_Type;  -- if region_bound, only valid for this range
      LBA_Bound_End   : LBA_Type;
      Use_Count : Unsigned_64;
      Checksum  : Unsigned_32;
      Pad      : Pad_4_Bytes;
   end record
   with Alignment => 8;

   type Crypto_State is record
      Initialized  : Boolean;
      Active_Keys  : Integer range 0 .. MAX_KEY_SLOTS;
      Encrypt_Count : Unsigned_64;
      Decrypt_Count : Unsigned_64;
      Tag_Fail_Count : Unsigned_32;
      Default_Key  : Integer range -1 .. MAX_KEY_SLOTS - 1;
   end record;

   State     : Crypto_State;
   Key_Slots : array (0 .. MAX_KEY_SLOTS - 1) of Crypto_Key_Slot;

   -- -------------------------------------------------------------------------
   -- Exported API
   -- -------------------------------------------------------------------------

   procedure Init
   with
      Export        => True,
      Convention    => C,
      External_Name => "cryptofs_init",
      Global        => (Output => (State, Key_Slots));

   -- Load a key into a slot
   function Load_Key
      (Slot_Id  : Unsigned_8;
       Cipher   : Unsigned_8;
       Key_Data : System.Address;
       Key_Len  : unsigned;
       Nonce    : System.Address;
       Nonce_Len : unsigned) return int
   with
      Export        => True,
      Convention    => C,
      External_Name => "cryptofs_load_key",
      Global        => (In_Out => (State, Key_Slots)),
      Pre           => Key_Len = KEY_SIZE and Nonce_Len = NONCE_SIZE;

   -- Bind a key to an LBA range
   function Bind_Key_To_Region
      (Slot_Id   : Unsigned_8;
       LBA_Start : LBA_Type;
       LBA_End   : LBA_Type) return int
   with
      Export        => True,
      Convention    => C,
      External_Name => "cryptofs_bind_key_region",
      Global        => (In_Out => Key_Slots),
      Pre           => LBA_End > LBA_Start;

   -- Encrypt one sector in-place
   function Encrypt_Sector
      (LBA    : LBA_Type;
       Buffer : System.Address;
       Tag    : System.Address) return int
   with
      Export        => True,
      Convention    => C,
      External_Name => "cryptofs_encrypt_sector",
      Global        => (In_Out => (State, Key_Slots));

   -- Decrypt one sector in-place, verify tag
   function Decrypt_Sector
      (LBA    : LBA_Type;
       Buffer : System.Address;
       Tag    : System.Address) return int
   with
      Export        => True,
      Convention    => C,
      External_Name => "cryptofs_decrypt_sector",
      Global        => (In_Out => (State, Key_Slots));

   -- Generate a sector authentication tag without encrypting
   function Tag_Sector
      (LBA    : LBA_Type;
       Buffer : System.Address;
       Tag    : System.Address) return int
   with
      Export        => True,
      Convention    => C,
      External_Name => "cryptofs_tag_sector",
      Global        => (In_Out => (State, Key_Slots));

   -- Verify sector tag
   function Verify_Tag
      (LBA    : LBA_Type;
       Buffer : System.Address;
       Tag    : System.Address) return int
   with
      Export        => True,
      Convention    => C,
      External_Name => "cryptofs_verify_tag",
      Global        => (In_Out => State,
                        Input  => Key_Slots);

   function Get_Tag_Fail_Count return Unsigned_32
   with
      Export        => True,
      Convention    => C,
      External_Name => "cryptofs_tag_fail_count",
      Global        => (Input => State);

   -- Internal
   function Find_Key_For_LBA (LBA : LBA_Type) return Integer
   with Global => (Input => (State, Key_Slots));

   function SipHash24
      (Key   : Key_Bytes;
       Data  : System.Address;
       Len   : Natural;
       LBA   : LBA_Type) return Unsigned_64
   with Global => null;

   procedure XOR_Stream
      (Key    : Key_Bytes;
       Nonce  : Nonce_T;
       LBA    : LBA_Type;
       Buffer : System.Address;
       Len    : Natural)
   with Global => null;

   function CRC32_Key_Slot (K : Crypto_Key_Slot) return Unsigned_32
   with Global => null;

end CryptoFS;