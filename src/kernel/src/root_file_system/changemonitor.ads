-- CosinusOS Change Monitor
-- changemonitor.ads — tracks disk mutations, detects unauthorized changes
-- Maintains a write journal and triggers alerts on suspicious activity

with Interfaces;   use Interfaces;
with Interfaces.C; use Interfaces.C;
with System;

package ChangeMonitor
   with SPARK_Mode => On
is

   MAX_JOURNAL_ENTRIES : constant := 512;
   MAX_WATCH_REGIONS   : constant := 16;
   ALERT_THRESHOLD     : constant := 10;   -- violations before hard lock
   BURST_WINDOW        : constant := 50;   -- ticks for burst detection
   BURST_LIMIT         : constant := 20;   -- max writes per burst window

   JOURNAL_MAGIC  : constant Unsigned_32 := 16#C051_4A4F#;  -- "COSJ"
   WATCH_MAGIC    : constant Unsigned_32 := 16#C051_5741#;  -- "COSW"

   -- Journal entry types
   JE_WRITE       : constant Unsigned_8 := 1;
   JE_READ        : constant Unsigned_8 := 2;
   JE_LOCK        : constant Unsigned_8 := 3;
   JE_UNLOCK      : constant Unsigned_8 := 4;
   JE_ALERT       : constant Unsigned_8 := 5;
   JE_VERIFY_FAIL : constant Unsigned_8 := 6;
   JE_BURST       : constant Unsigned_8 := 7;
   JE_ROLLBACK    : constant Unsigned_8 := 8;

   ERR_OK         : constant int := 0;
   ERR_ALERT      : constant int := -1;
   ERR_BURST      : constant int := -2;
   ERR_WATCH_FULL : constant int := -3;
   ERR_NOT_FOUND  : constant int := -4;

   subtype LBA_Type   is Unsigned_64;
   subtype Ring_Level is Unsigned_8 range 0 .. 3;

   -- One journal entry — records a disk mutation event
   type Journal_Entry is record
      Magic       : Unsigned_32;
      Entry_Type  : Unsigned_8;
      Ring        : Ring_Level;
      Region_Id   : Unsigned_8;
      Flags       : Unsigned_8;
      LBA         : LBA_Type;
      Sector_Count: Unsigned_32;
      Tick        : Unsigned_64;
      Before_Hash : Unsigned_32;  -- FNV1a of sector before write
      After_Hash  : Unsigned_32;  -- FNV1a of sector after write
      Caller_Id   : Unsigned_32;  -- opaque caller identifier
   end record
   with Alignment => 8;

   -- Watch region — triggers alerts when written to unexpectedly
   type Watch_Region is record
      Magic       : Unsigned_32;
      LBA_Start   : LBA_Type;
      LBA_End     : LBA_Type;
      Expected_Hash : Unsigned_32;  -- expected content hash
      Alert_Count : Unsigned_32;
      Active      : Boolean;
      Strict      : Boolean;  -- strict=True means any write triggers alert
      _Pad        : array (0 .. 1) of Unsigned_8;
   end record;

   -- Monitor state
   type Monitor_State is record
      Initialized      : Boolean;
      Tick             : Unsigned_64;
      Journal_Head     : Integer range 0 .. MAX_JOURNAL_ENTRIES - 1;
      Journal_Count    : Unsigned_32;
      Total_Writes     : Unsigned_64;
      Total_Alerts     : Unsigned_32;
      Burst_Count      : Unsigned_32;
      Burst_Window_Start : Unsigned_64;
      Hard_Lock        : Boolean;
      Watch_Count      : Integer range 0 .. MAX_WATCH_REGIONS;
   end record;

   State   : Monitor_State;
   Journal : array (0 .. MAX_JOURNAL_ENTRIES - 1) of Journal_Entry;
   Watches : array (0 .. MAX_WATCH_REGIONS  - 1) of Watch_Region;

   -- -------------------------------------------------------------------------
   -- Exported API
   -- -------------------------------------------------------------------------

   procedure Init
   with
      Export        => True,
      Convention    => C,
      External_Name => "change_monitor_init",
      Global        => (Output => (State, Journal, Watches));

   procedure Tick
   with
      Export        => True,
      Convention    => C,
      External_Name => "change_monitor_tick",
      Global        => (In_Out => State);

   -- Record a write operation — returns ERR_ALERT if suspicious
   function Record_Write
      (LBA         : LBA_Type;
       Count       : Unsigned_32;
       Ring        : Ring_Level;
       Before_Hash : Unsigned_32;
       After_Hash  : Unsigned_32) return int
   with
      Export        => True,
      Convention    => C,
      External_Name => "change_monitor_record_write",
      Global        => (In_Out => (State, Journal, Watches)),
      Pre           => Count > 0;

   -- Record a read
   procedure Record_Read
      (LBA   : LBA_Type;
       Count : Unsigned_32;
       Ring  : Ring_Level)
   with
      Export        => True,
      Convention    => C,
      External_Name => "change_monitor_record_read",
      Global        => (In_Out => (State, Journal));

   -- Add a watch region
   function Add_Watch
      (LBA_Start     : LBA_Type;
       LBA_End       : LBA_Type;
       Expected_Hash : Unsigned_32;
       Strict        : int) return int
   with
      Export        => True,
      Convention    => C,
      External_Name => "change_monitor_add_watch",
      Global        => (In_Out => (State, Watches)),
      Pre           => LBA_End > LBA_Start;

   -- Remove a watch region
   function Remove_Watch (LBA_Start : LBA_Type) return int
   with
      Export        => True,
      Convention    => C,
      External_Name => "change_monitor_remove_watch",
      Global        => (In_Out => (State, Watches));

   -- Check if LBA is in any watch region
   function Check_Watch (LBA : LBA_Type) return int
   with
      Export        => True,
      Convention    => C,
      External_Name => "change_monitor_check_watch",
      Global        => (Input => (State, Watches));

   -- Get total alert count
   function Get_Alert_Count return Unsigned_32
   with
      Export        => True,
      Convention    => C,
      External_Name => "change_monitor_alert_count",
      Global        => (Input => State);

   -- Is monitor in hard lock state?
   function Is_Hard_Locked return int
   with
      Export        => True,
      Convention    => C,
      External_Name => "change_monitor_is_locked",
      Global        => (Input => State);

   -- Get last N journal entries into caller buffer
   function Dump_Journal
      (Buffer    : System.Address;
       Max_Count : unsigned) return int
   with
      Export        => True,
      Convention    => C,
      External_Name => "change_monitor_dump_journal",
      Global        => (Input => (State, Journal));

   -- Internal
   procedure Append_Journal
      (Entry_Type  : Unsigned_8;
       Ring        : Ring_Level;
       LBA         : LBA_Type;
       Count       : Unsigned_32;
       Before_Hash : Unsigned_32;
       After_Hash  : Unsigned_32)
   with Global => (In_Out => (State, Journal));

   function Check_Burst return Boolean
   with Global => (In_Out => State);

   function FNV1a (Data : System.Address; Len : Natural) return Unsigned_32
   with Global => null;

end ChangeMonitor;