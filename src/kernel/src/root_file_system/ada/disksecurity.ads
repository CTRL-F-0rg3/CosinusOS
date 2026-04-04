-- CosinusOS Disk Security Layer
-- disksecurity.ads — main security spec
-- Provides: sector access control, write protection, region locking
-- Used by: Rust root_file_system via glue.c FFI

with Interfaces;   use Interfaces;
with Interfaces.C; use Interfaces.C;
with System;

package DiskSecurity
   with SPARK_Mode => On
is

   -- -------------------------------------------------------------------------
   -- Constants
   -- -------------------------------------------------------------------------

   SECTOR_SIZE        : constant := 512;
   MAX_REGIONS        : constant := 32;
   MAX_AUDIT_ENTRIES  : constant := 1024;
   MAX_KEYS           : constant := 8;

   MAGIC_REGION_VALID : constant Unsigned_32 := 16#C051_5EC0#;
   MAGIC_KEY_VALID    : constant Unsigned_32 := 16#C051_4B45#;

   -- Security levels — higher = more restrictive
   LEVEL_OPEN         : constant := 0;  -- any ring can read/write
   LEVEL_KERNEL_ONLY  : constant := 1;  -- ring 0 only
   LEVEL_SIGNED_ONLY  : constant := 2;  -- must present valid HMAC
   LEVEL_LOCKED       : constant := 3;  -- no writes permitted ever

   -- Region types
   REGION_KERNEL      : constant := 0;
   REGION_DEVSPACE    : constant := 1;
   REGION_FSSERVER    : constant := 2;
   REGION_USERSPACE   : constant := 3;
   REGION_DATA        : constant := 4;
   REGION_RESERVED    : constant := 5;

   -- Operation codes for audit log
   OP_READ            : constant Unsigned_8 := 16#01#;
   OP_WRITE           : constant Unsigned_8 := 16#02#;
   OP_VERIFY          : constant Unsigned_8 := 16#03#;
   OP_LOCK            : constant Unsigned_8 := 16#04#;
   OP_UNLOCK          : constant Unsigned_8 := 16#05#;
   OP_AUTH            : constant Unsigned_8 := 16#06#;
   OP_INTEGRITY       : constant Unsigned_8 := 16#07#;
   OP_VIOLATION       : constant Unsigned_8 := 16#FF#;

   -- Error codes returned to Rust via C ABI
   ERR_OK             : constant int := 0;
   ERR_PERMISSION     : constant int := -1;
   ERR_REGION_FAULT   : constant int := -2;
   ERR_INTEGRITY      : constant int := -3;
   ERR_AUTH_FAIL      : constant int := -4;
   ERR_LOCKED         : constant int := -5;
   ERR_BOUNDS         : constant int := -6;
   ERR_INVALID_KEY    : constant int := -7;
   ERR_TAMPER         : constant int := -8;

   -- -------------------------------------------------------------------------
   -- Types
   -- -------------------------------------------------------------------------

   subtype LBA_Type     is Unsigned_64;
   subtype Sector_Count is Unsigned_32;
   subtype Ring_Level   is Unsigned_8 range 0 .. 3;
   subtype Region_Index is Integer range 0 .. MAX_REGIONS - 1;
   subtype Key_Index    is Integer range 0 .. MAX_KEYS - 1;
   subtype Security_Level is Integer range 0 .. 3;

   type HMAC_Tag is array (0 .. 31) of Unsigned_8;
   type AES_Key  is array (0 .. 31) of Unsigned_8;  -- 256-bit
   type Hash_256 is array (0 .. 31) of Unsigned_8;
   type Nonce_96    is array (0 .. 11) of Unsigned_8;
   type Pad_6_Bytes is array (0 .. 5)  of Unsigned_8;
   type Pad_4_Bytes is array (0 .. 3)  of Unsigned_8;

   -- Disk region descriptor — one per protected segment
   type Region_Descriptor is record
      Magic        : Unsigned_32;      -- MAGIC_REGION_VALID when active
      Region_Type  : Unsigned_8;
      Sec_Level    : Unsigned_8;
      Flags        : Unsigned_16;      -- bit 0=locked, 1=encrypted, 2=verified
      LBA_Start    : LBA_Type;
      LBA_End      : LBA_Type;
      Sector_Count : Unsigned_32;
      Checksum     : Unsigned_32;      -- CRC32 of this descriptor
      Content_Hash : Hash_256;         -- SHA-256 of region content at install time
      Write_Count  : Unsigned_64;      -- monotonic write counter
      Last_Writer  : Ring_Level;
      Pad         : Pad_6_Bytes;
   end record
   with Size => 512, Alignment => 8;  -- exactly one sector

   -- Key slot — holds encryption/auth key material
   type Key_Slot is record
      Magic       : Unsigned_32;
      Key_Id      : Unsigned_8;
      Algorithm   : Unsigned_8;   -- 0=AES256-GCM, 1=ChaCha20, 2=HMAC-SHA256
      Flags       : Unsigned_16;  -- bit 0=active, 1=revoked
      Key_Data    : AES_Key;
      Auth_Tag    : HMAC_Tag;
      Nonce       : Nonce_96;
      Generation  : Unsigned_32;
      Pad        : Pad_4_Bytes;
   end record
   with Size => 512, Alignment => 8;

   -- Audit log entry
   type Audit_Entry is record
      Timestamp   : Unsigned_64;   -- monotonic tick counter from kernel
      Operation   : Unsigned_8;
      Ring        : Ring_Level;
      Region      : Unsigned_8;
      Result_Code : Unsigned_8;
      LBA         : LBA_Type;
      Count       : Unsigned_32;
      Caller_Hash : Unsigned_32;   -- hash of caller identity
   end record
   with Size => 192, Alignment => 8;

   -- Global security state
   type Security_State is record
      Initialized    : Boolean;
      Boot_Count     : Unsigned_32;
      Violation_Count: Unsigned_32;
      Active_Regions : Integer range 0 .. MAX_REGIONS;
      Active_Keys    : Integer range 0 .. MAX_KEYS;
      Audit_Head     : Integer range 0 .. MAX_AUDIT_ENTRIES - 1;
      Audit_Count    : Unsigned_32;
      Tamper_Detected: Boolean;
      Lock_All       : Boolean;
   end record;

   -- -------------------------------------------------------------------------
   -- Global state (one instance, kernel-owned)
   -- -------------------------------------------------------------------------

   State   : Security_State;
   Regions : array (Region_Index) of Region_Descriptor;
   Keys    : array (Key_Index)    of Key_Slot;
   Audit   : array (0 .. MAX_AUDIT_ENTRIES - 1) of Audit_Entry;

   -- -------------------------------------------------------------------------
   -- Primary API — exported to C/Rust
   -- -------------------------------------------------------------------------

   procedure Init
   with
      Export        => True,
      Convention    => C,
      External_Name => "disk_security_init",
      Global        => (Output => (State, Regions, Keys, Audit));

   function Check_Access
      (LBA        : LBA_Type;
       Count      : Sector_Count;
       Operation  : Unsigned_8;
       Ring       : Ring_Level) return int
   with
      Export        => True,
      Convention    => C,
      External_Name => "disk_security_check_access",
      Global        => (In_Out => (State, Audit),
                        Input  => Regions),
      Pre           => Count > 0 and Count <= 65536;

   function Register_Region
      (LBA_Start   : LBA_Type;
       LBA_End     : LBA_Type;
       Region_Type : Unsigned_8;
       Sec_Level   : Unsigned_8) return int
   with
      Export        => True,
      Convention    => C,
      External_Name => "disk_security_register_region",
      Global        => (In_Out => (State, Regions)),
      Pre           => LBA_End > LBA_Start
                       and Sec_Level <= 3;

   function Lock_Region
      (LBA_Start : LBA_Type) return int
   with
      Export        => True,
      Convention    => C,
      External_Name => "disk_security_lock_region",
      Global        => (Output => (State, Regions, Audit));

   function Unlock_Region
      (LBA_Start : LBA_Type;
       Auth_Tag  : System.Address;
       Tag_Len   : unsigned) return int
   with
      Export        => True,
      Convention    => C,
      External_Name => "disk_security_unlock_region",
      Global        => (In_Out => (State, Regions, Audit)),
      Pre           => Tag_Len = 32;

   function Set_Content_Hash
      (LBA_Start : LBA_Type;
       Hash      : System.Address;
       Hash_Len  : unsigned) return int
   with
      Export        => True,
      Convention    => C,
      External_Name => "disk_security_set_content_hash",
      Global        => (In_Out => Regions),
      Pre           => Hash_Len = 32;

   function Verify_Region
      (LBA_Start    : LBA_Type;
       Sector_Data  : System.Address;
       Data_Len     : unsigned) return int
   with
      Export        => True,
      Convention    => C,
      External_Name => "disk_security_verify_region",
      Global        => (In_Out => (State, Audit),
                        Input  => Regions);

   function Get_Violation_Count return Unsigned_32
   with
      Export        => True,
      Convention    => C,
      External_Name => "disk_security_get_violation_count",
      Global        => (Input => State);

   function Is_Tampered return int
   with
      Export        => True,
      Convention    => C,
      External_Name => "disk_security_is_tampered",
      Global        => (Input => State);

   procedure Emergency_Lock_All
   with
      Export        => True,
      Convention    => C,
      External_Name => "disk_security_emergency_lock",
      Global        => (In_Out => (State, Regions, Audit));

   -- -------------------------------------------------------------------------
   -- Internal helpers (not exported)
   -- -------------------------------------------------------------------------

   function Find_Region (LBA : LBA_Type) return Integer
   with Global => (Input => (State, Regions));

   function CRC32_Region (R : Region_Descriptor) return Unsigned_32
   with Global => null;

   procedure Log_Audit
      (Op     : Unsigned_8;
       Ring   : Ring_Level;
       Region : Unsigned_8;
       Result : Unsigned_8;
       LBA    : LBA_Type;
       Count  : Unsigned_32)
   with Global => (In_Out => (State, Audit));

   function Verify_Descriptor (R : Region_Descriptor) return Boolean
   with Global => null;

end DiskSecurity;