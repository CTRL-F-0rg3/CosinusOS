-- CosinusOS Disk Auth Layer
-- diskauth.ads — authentication and authorization for disk operations
-- Verifies write tokens, manages session keys, enforces write permits

with Interfaces;   use Interfaces;
with Interfaces.C; use Interfaces.C;
with System;

package DiskAuth
   with SPARK_Mode => On
is

   MAX_SESSIONS    : constant := 16;
   MAX_PERMITS     : constant := 64;
   SESSION_TIMEOUT : constant := 1000;  -- ticks

   TOKEN_SIZE      : constant := 32;
   PERMIT_MAGIC    : constant Unsigned_32 := 16#C051_5057#;  -- "COSP"
   SESSION_MAGIC   : constant Unsigned_32 := 16#C051_5345#;  -- "COSE"

   -- Permit flags
   PERMIT_READ     : constant Unsigned_16 := 16#0001#;
   PERMIT_WRITE    : constant Unsigned_16 := 16#0002#;
   PERMIT_VERIFY   : constant Unsigned_16 := 16#0004#;
   PERMIT_ADMIN    : constant Unsigned_16 := 16#0100#;  -- kernel-only

   -- Session states
   SESSION_INVALID : constant Unsigned_8 := 0;
   SESSION_ACTIVE  : constant Unsigned_8 := 1;
   SESSION_EXPIRED : constant Unsigned_8 := 2;
   SESSION_REVOKED : constant Unsigned_8 := 3;

   ERR_OK          : constant int := 0;
   ERR_NO_PERMIT   : constant int := -1;
   ERR_EXPIRED     : constant int := -2;
   ERR_REVOKED     : constant int := -3;
   ERR_BAD_TOKEN   : constant int := -4;
   ERR_NO_SESSION  : constant int := -5;
   ERR_FULL        : constant int := -6;
   ERR_DENIED      : constant int := -7;

   subtype LBA_Type     is Unsigned_64;
   subtype Session_Id   is Unsigned_32;
   subtype Token_Bytes  is array (0 .. TOKEN_SIZE - 1) of Unsigned_8;
   subtype Ring_Level   is Unsigned_8 range 0 .. 3;

   -- Write permit — authorizes a specific LBA range operation
   type Write_Permit is record
      Magic       : Unsigned_32;
      Session     : Session_Id;
      LBA_Start   : LBA_Type;
      LBA_End     : LBA_Type;
      Flags       : Unsigned_16;
      Ring        : Ring_Level;
      Used        : Boolean;
      Tick_Issued : Unsigned_64;
      Tick_Expiry : Unsigned_64;
      Token       : Token_Bytes;
      Checksum    : Unsigned_32;
   end record;

   -- Auth session
   type Auth_Session is record
      Magic        : Unsigned_32;
      Id           : Session_Id;
      State        : Unsigned_8;
      Ring         : Ring_Level;
      Tick_Created : Unsigned_64;
      Tick_Last    : Unsigned_64;
      Token        : Token_Bytes;
      Permit_Count : Unsigned_8;
      Write_Count  : Unsigned_32;
      _Pad         : array (0 .. 1) of Unsigned_8;
   end record;

   -- Global auth state
   type Auth_State is record
      Initialized    : Boolean;
      Tick           : Unsigned_64;
      Session_Count  : Integer range 0 .. MAX_SESSIONS;
      Permit_Count   : Integer range 0 .. MAX_PERMITS;
      Total_Denials  : Unsigned_32;
      Total_Permits  : Unsigned_32;
   end record;

   State    : Auth_State;
   Sessions : array (0 .. MAX_SESSIONS - 1) of Auth_Session;
   Permits  : array (0 .. MAX_PERMITS  - 1) of Write_Permit;

   -- -------------------------------------------------------------------------
   -- Exported API
   -- -------------------------------------------------------------------------

   procedure Init
   with
      Export        => True,
      Convention    => C,
      External_Name => "disk_auth_init",
      Global        => (Output => (State, Sessions, Permits));

   -- Advance internal tick counter (called by kernel timer)
   procedure Tick
   with
      Export        => True,
      Convention    => C,
      External_Name => "disk_auth_tick",
      Global        => (In_Out => (State, Sessions));

   -- Open a new auth session, returns session ID or negative error
   function Open_Session
      (Ring      : Ring_Level;
       Token     : System.Address;
       Token_Len : unsigned) return int
   with
      Export        => True,
      Convention    => C,
      External_Name => "disk_auth_open_session",
      Global        => (In_Out => (State, Sessions)),
      Pre           => Token_Len = TOKEN_SIZE;

   -- Close and invalidate a session
   function Close_Session (Id : Session_Id) return int
   with
      Export        => True,
      Convention    => C,
      External_Name => "disk_auth_close_session",
      Global        => (In_Out => (State, Sessions));

   -- Issue a write permit for an LBA range
   function Issue_Permit
      (Session   : Session_Id;
       LBA_Start : LBA_Type;
       LBA_End   : LBA_Type;
       Flags     : Unsigned_16) return int
   with
      Export        => True,
      Convention    => C,
      External_Name => "disk_auth_issue_permit",
      Global        => (In_Out => (State, Sessions, Permits)),
      Pre           => LBA_End > LBA_Start;

   -- Check if an LBA write operation has a valid permit
   function Check_Permit
      (LBA_Start : LBA_Type;
       Count     : Unsigned_32;
       Ring      : Ring_Level;
       Flags     : Unsigned_16) return int
   with
      Export        => True,
      Convention    => C,
      External_Name => "disk_auth_check_permit",
      Global        => (In_Out => (State, Permits),
                        Input  => Sessions),
      Pre           => Count > 0;

   -- Revoke all permits for a session
   function Revoke_Session_Permits (Id : Session_Id) return int
   with
      Export        => True,
      Convention    => C,
      External_Name => "disk_auth_revoke_permits",
      Global        => (In_Out => (State, Permits));

   -- Kernel-only: issue an unconditional admin permit (ring 0 only)
   function Admin_Permit
      (LBA_Start : LBA_Type;
       LBA_End   : LBA_Type) return int
   with
      Export        => True,
      Convention    => C,
      External_Name => "disk_auth_admin_permit",
      Global        => (In_Out => (State, Sessions, Permits)),
      Pre           => LBA_End > LBA_Start;

   -- Query active session count
   function Active_Sessions return int
   with
      Export        => True,
      Convention    => C,
      External_Name => "disk_auth_active_sessions",
      Global        => (Input => State);

   -- Internal helpers
   function Find_Session (Id : Session_Id) return Integer
   with Global => (Input => (State, Sessions));

   function Find_Free_Session return Integer
   with Global => (Input => (State, Sessions));

   function Find_Free_Permit return Integer
   with Global => (Input => (State, Permits));

   function Token_Match
      (A : Token_Bytes;
       B : Token_Bytes) return Boolean
   with Global => null;

   function Session_Valid (S : Auth_Session) return Boolean
   with Global => (Input => State);

   function CRC32_Permit (P : Write_Permit) return Unsigned_32
   with Global => null;

end DiskAuth;