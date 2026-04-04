-- CosinusOS Change Monitor
-- changemonitor.adb — implementation

package body ChangeMonitor
   with SPARK_Mode => On
is

   -- -------------------------------------------------------------------------
   -- FNV-1a 32-bit
   -- -------------------------------------------------------------------------

   function FNV1a (Data : System.Address; Len : Natural) return Unsigned_32 is
      type BA is array (Natural range <>) of Unsigned_8;
      B : BA (0 .. Len - 1) with Address => Data, Import => True;
      H : Unsigned_32 := 16#811C9DC5#;
   begin
      for I in 0 .. Len - 1 loop
         H := H xor Unsigned_32 (B (I));
         H := H * 16#01000193#;
      end loop;
      return H;
   end FNV1a;

   -- -------------------------------------------------------------------------
   -- Init
   -- -------------------------------------------------------------------------

   procedure Init is
   begin
      State.Initialized        := False;
      State.Tick               := 0;
      State.Journal_Head       := 0;
      State.Journal_Count      := 0;
      State.Total_Writes       := 0;
      State.Total_Alerts       := 0;
      State.Burst_Count        := 0;
      State.Burst_Window_Start := 0;
      State.Hard_Lock          := False;
      State.Watch_Count        := 0;

      for I in 0 .. MAX_JOURNAL_ENTRIES - 1 loop
         Journal (I).Magic        := 0;
         Journal (I).Entry_Type   := 0;
         Journal (I).Ring         := 0;
         Journal (I).Region_Id    := 0;
         Journal (I).Flags        := 0;
         Journal (I).LBA          := 0;
         Journal (I).Sector_Count := 0;
         Journal (I).Tick         := 0;
         Journal (I).Before_Hash  := 0;
         Journal (I).After_Hash   := 0;
         Journal (I).Caller_Id    := 0;
      end loop;

      for I in 0 .. MAX_WATCH_REGIONS - 1 loop
         Watches (I).Magic         := 0;
         Watches (I).LBA_Start     := 0;
         Watches (I).LBA_End       := 0;
         Watches (I).Expected_Hash := 0;
         Watches (I).Alert_Count   := 0;
         Watches (I).Active        := False;
         Watches (I).Strict        := False;
      end loop;

      State.Initialized := True;
   end Init;

   -- -------------------------------------------------------------------------
   -- Tick
   -- -------------------------------------------------------------------------

   procedure Tick is
   begin
      if State.Initialized then
         State.Tick := State.Tick + 1;
         -- Reset burst window if enough time has passed
         if State.Tick - State.Burst_Window_Start > BURST_WINDOW then
            State.Burst_Count        := 0;
            State.Burst_Window_Start := State.Tick;
         end if;
      end if;
   end Tick;

   -- -------------------------------------------------------------------------
   -- Append_Journal
   -- -------------------------------------------------------------------------

   procedure Append_Journal
      (Entry_Type  : Unsigned_8;
       Ring        : Ring_Level;
       LBA         : LBA_Type;
       Count       : Unsigned_32;
       Before_Hash : Unsigned_32;
       After_Hash  : Unsigned_32)
   is
      Idx : constant Integer := State.Journal_Head;
   begin
      Journal (Idx).Magic        := JOURNAL_MAGIC;
      Journal (Idx).Entry_Type   := Entry_Type;
      Journal (Idx).Ring         := Ring;
      Journal (Idx).Region_Id    := 0;
      Journal (Idx).Flags        := 0;
      Journal (Idx).LBA          := LBA;
      Journal (Idx).Sector_Count := Count;
      Journal (Idx).Tick         := State.Tick;
      Journal (Idx).Before_Hash  := Before_Hash;
      Journal (Idx).After_Hash   := After_Hash;
      Journal (Idx).Caller_Id    := 0;

      if State.Journal_Head < MAX_JOURNAL_ENTRIES - 1 then
         State.Journal_Head := State.Journal_Head + 1;
      else
         State.Journal_Head := 0;
      end if;

      if State.Journal_Count < Unsigned_32'Last then
         State.Journal_Count := State.Journal_Count + 1;
      end if;
   end Append_Journal;

   -- -------------------------------------------------------------------------
   -- Check_Burst — detect write storms
   -- -------------------------------------------------------------------------

   function Check_Burst return Boolean is
   begin
      State.Burst_Count := State.Burst_Count + 1;
      if State.Tick - State.Burst_Window_Start > BURST_WINDOW then
         State.Burst_Count        := 1;
         State.Burst_Window_Start := State.Tick;
         return False;
      end if;
      return State.Burst_Count > BURST_LIMIT;
   end Check_Burst;

   -- -------------------------------------------------------------------------
   -- Record_Write
   -- -------------------------------------------------------------------------

   function Record_Write
      (LBA         : LBA_Type;
       Count       : Unsigned_32;
       Ring        : Ring_Level;
       Before_Hash : Unsigned_32;
       After_Hash  : Unsigned_32) return int
   is
      In_Watch : Boolean := False;
      Strict   : Boolean := False;
   begin
      if not State.Initialized then
         return ERR_OK;
      end if;

      if State.Hard_Lock then
         return ERR_ALERT;
      end if;

      -- Check burst
      if Check_Burst then
         Append_Journal (JE_BURST, Ring, LBA, Count, Before_Hash, After_Hash);
         State.Total_Alerts := State.Total_Alerts + 1;
         if State.Total_Alerts >= ALERT_THRESHOLD then
            State.Hard_Lock := True;
         end if;
         return ERR_BURST;
      end if;

      -- Check watch regions
      for I in 0 .. State.Watch_Count - 1 loop
         if Watches (I).Active
            and Watches (I).Magic = WATCH_MAGIC
            and LBA >= Watches (I).LBA_Start
            and LBA < Watches (I).LBA_End
         then
            In_Watch := True;
            Strict   := Watches (I).Strict;
            Watches (I).Alert_Count := Watches (I).Alert_Count + 1;

            -- If strict, any write is an alert
            if Strict then
               Append_Journal (JE_ALERT, Ring, LBA, Count, Before_Hash, After_Hash);
               State.Total_Alerts := State.Total_Alerts + 1;
               if State.Total_Alerts >= ALERT_THRESHOLD then
                  State.Hard_Lock := True;
               end if;
               return ERR_ALERT;
            end if;

            -- Non-strict: alert only if hash changed unexpectedly
            if Watches (I).Expected_Hash /= 0
               and After_Hash /= Watches (I).Expected_Hash
            then
               Append_Journal (JE_VERIFY_FAIL, Ring, LBA, Count, Before_Hash, After_Hash);
               State.Total_Alerts := State.Total_Alerts + 1;
               return ERR_ALERT;
            end if;
         end if;
      end loop;

      pragma Unreferenced (In_Watch);

      -- Normal write — just journal it
      Append_Journal (JE_WRITE, Ring, LBA, Count, Before_Hash, After_Hash);
      State.Total_Writes := State.Total_Writes + 1;
      return ERR_OK;
   end Record_Write;

   -- -------------------------------------------------------------------------
   -- Record_Read
   -- -------------------------------------------------------------------------

   procedure Record_Read
      (LBA   : LBA_Type;
       Count : Unsigned_32;
       Ring  : Ring_Level)
   is
   begin
      if State.Initialized then
         Append_Journal (JE_READ, Ring, LBA, Count, 0, 0);
      end if;
   end Record_Read;

   -- -------------------------------------------------------------------------
   -- Add_Watch
   -- -------------------------------------------------------------------------

   function Add_Watch
      (LBA_Start     : LBA_Type;
       LBA_End       : LBA_Type;
       Expected_Hash : Unsigned_32;
       Strict        : int) return int
   is
      Idx : Integer;
   begin
      if not State.Initialized then
         return ERR_NOT_FOUND;
      end if;

      if State.Watch_Count >= MAX_WATCH_REGIONS then
         return ERR_WATCH_FULL;
      end if;

      -- Find a free slot
      Idx := -1;
      for I in 0 .. MAX_WATCH_REGIONS - 1 loop
         if not Watches (I).Active then
            Idx := I;
            exit;
         end if;
      end loop;

      if Idx = -1 then
         return ERR_WATCH_FULL;
      end if;

      Watches (Idx).Magic         := WATCH_MAGIC;
      Watches (Idx).LBA_Start     := LBA_Start;
      Watches (Idx).LBA_End       := LBA_End;
      Watches (Idx).Expected_Hash := Expected_Hash;
      Watches (Idx).Alert_Count   := 0;
      Watches (Idx).Active        := True;
      Watches (Idx).Strict        := Strict /= 0;
      for J in 0 .. 1 loop
         Watches (Idx).Pad (J) := 0;
      end loop;

      State.Watch_Count := State.Watch_Count + 1;
      return ERR_OK;
   end Add_Watch;

   -- -------------------------------------------------------------------------
   -- Remove_Watch
   -- -------------------------------------------------------------------------

   function Remove_Watch (LBA_Start : LBA_Type) return int is
   begin
      for I in 0 .. State.Watch_Count - 1 loop
         if Watches (I).Active
            and Watches (I).Magic = WATCH_MAGIC
            and Watches (I).LBA_Start = LBA_Start
         then
            Watches (I).Active := False;
            Watches (I).Magic  := 0;
            if State.Watch_Count > 0 then
               State.Watch_Count := State.Watch_Count - 1;
            end if;
            return ERR_OK;
         end if;
      end loop;
      return ERR_NOT_FOUND;
   end Remove_Watch;

   -- -------------------------------------------------------------------------
   -- Check_Watch
   -- -------------------------------------------------------------------------

   function Check_Watch (LBA : LBA_Type) return int is
   begin
      for I in 0 .. State.Watch_Count - 1 loop
         if Watches (I).Active
            and Watches (I).Magic = WATCH_MAGIC
            and LBA >= Watches (I).LBA_Start
            and LBA < Watches (I).LBA_End
         then
            return 1;
         end if;
      end loop;
      return 0;
   end Check_Watch;

   -- -------------------------------------------------------------------------
   -- Get_Alert_Count
   -- -------------------------------------------------------------------------

   function Get_Alert_Count return Unsigned_32 is
   begin
      return State.Total_Alerts;
   end Get_Alert_Count;

   -- -------------------------------------------------------------------------
   -- Is_Hard_Locked
   -- -------------------------------------------------------------------------

   function Is_Hard_Locked return int is
   begin
      if State.Hard_Lock then return 1; else return 0; end if;
   end Is_Hard_Locked;

   -- -------------------------------------------------------------------------
   -- Dump_Journal
   -- -------------------------------------------------------------------------

   function Dump_Journal
      (Buffer    : System.Address;
       Max_Count : unsigned) return int
   is
      type JE_Array is array (Natural range <>) of Journal_Entry;
      Dst : JE_Array (0 .. Natural (Max_Count) - 1)
         with Address => Buffer, Import => True;
      N   : Natural := Natural'Min
               (Natural (Max_Count),
                Natural (State.Journal_Count));
      Src_Idx : Integer;
   begin
      for I in 0 .. N - 1 loop
         -- Read backwards from head
         Src_Idx := State.Journal_Head - 1 - I;
         if Src_Idx < 0 then
            Src_Idx := Src_Idx + MAX_JOURNAL_ENTRIES;
         end if;
         Dst (I) := Journal (Src_Idx);
      end loop;
      return int (N);
   end Dump_Journal;

end ChangeMonitor;