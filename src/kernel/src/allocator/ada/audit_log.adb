-- CosinusOS — allocator/ada/audit_log.adb

with System.Machine_Code;     use System.Machine_Code;
with System.Storage_Elements; use System.Storage_Elements;

package body Audit_Log is
   use type System.Address;

   -- -------------------------------------------------------------------------
   -- Serial helpers
   -- -------------------------------------------------------------------------
   procedure Out_Char (C : Character) is
   begin
      Asm ("outb %0, $0xe9",
           Inputs => Character'Asm_Input ("a", C), Volatile => True);
   end Out_Char;

   procedure Out_Str (S : String) is
   begin
      for Ch of S loop Out_Char (Ch); end loop;
   end Out_Str;

   procedure Out_Hex (V : Unsigned_64) is
      Hex_Chars : constant String := "0123456789abcdef";
      Tmp       : Unsigned_64 := V;
      Buf       : String (1 .. 16);
   begin
      for I in reverse 1 .. 16 loop
         Buf (I) := Hex_Chars (Integer (Tmp and 16#F#) + 1);
         Tmp := Shift_Right (Tmp, 4);
      end loop;
      Out_Str (Buf);
   end Out_Hex;

   procedure Out_Dec (V : Unsigned_64) is
      Tmp : Unsigned_64 := V;
      Buf : String (1 .. 20);
      Pos : Integer := 20;
   begin
      if Tmp = 0 then Out_Char ('0'); return; end if;
      while Tmp > 0 loop
         Buf (Pos) := Character'Val (Character'Pos ('0') + Integer (Tmp mod 10));
         Tmp := Tmp / 10;
         Pos := Pos - 1;
      end loop;
      Out_Str (Buf (Pos + 1 .. 20));
   end Out_Dec;

   -- -------------------------------------------------------------------------
   -- Ring buffer
   -- -------------------------------------------------------------------------
   type Entry_Kind is (Kind_Alloc, Kind_Free);

   type Audit_Entry is record
      Ptr     : System.Address := System.Null_Address;
      Size    : Unsigned_64    := 0;
      Is_Slab : Boolean        := False;
      Kind    : Entry_Kind     := Kind_Alloc;
   end record;

   type Entry_Array is array (0 .. AUDIT_SIZE - 1) of Audit_Entry;

   Ring       : Entry_Array;
   Ring_Head  : Integer    := 0;
   N_Allocs   : Unsigned_64 := 0;
   N_Frees    : Unsigned_64 := 0;
   Live_Total : Unsigned_64 := 0;

   procedure Push (E : Audit_Entry) is
   begin
      Ring (Ring_Head) := E;
      Ring_Head := (Ring_Head + 1) mod AUDIT_SIZE;
   end Push;

   -- -------------------------------------------------------------------------
   -- Public
   -- -------------------------------------------------------------------------
   procedure Log_Alloc (Ptr : System.Address; Size : Unsigned_64; Is_Slab : C_Bool) is
   begin
      Push ((Ptr, Size, Is_Slab /= 0, Kind_Alloc));
      N_Allocs   := N_Allocs + 1;
      Live_Total := Live_Total + Size;
   end Log_Alloc;

   procedure Log_Free (Ptr : System.Address; Size : Unsigned_64; Is_Slab : C_Bool) is
   begin
      Push ((Ptr, Size, Is_Slab /= 0, Kind_Free));
      N_Frees := N_Frees + 1;
      if Live_Total >= Size then
         Live_Total := Live_Total - Size;
      else
         Live_Total := 0;
      end if;
   end Log_Free;

   procedure Get_Stats
     (Total_Allocs : out Unsigned_64;
      Total_Frees  : out Unsigned_64;
      Live_Bytes   : out Unsigned_64)
   is
   begin
      Total_Allocs := N_Allocs;
      Total_Frees  := N_Frees;
      Live_Bytes   := Live_Total;
   end Get_Stats;

   procedure Dump_Last (N : Unsigned_32) is
      Count : Integer := Integer (N);
      Idx   : Integer;
      E     : Audit_Entry;
   begin
      if Count > AUDIT_SIZE then Count := AUDIT_SIZE; end if;
      Out_Str ("--- audit dump last="); Out_Dec (Unsigned_64 (Count));
      Out_Str (" allocs="); Out_Dec (N_Allocs);
      Out_Str (" frees="); Out_Dec (N_Frees);
      Out_Str (" live="); Out_Dec (Live_Total);
      Out_Str ("B ---"); Out_Char (ASCII.LF);
      for I in 1 .. Count loop
         Idx := (Ring_Head - I + AUDIT_SIZE) mod AUDIT_SIZE;
         E   := Ring (Idx);
         if E.Ptr /= System.Null_Address then
            if E.Kind = Kind_Alloc then Out_Str ("  A ");
            else                        Out_Str ("  F ");
            end if;
            if E.Is_Slab then Out_Str ("slab ");
            else              Out_Str ("budy ");
            end if;
            Out_Str ("0x"); Out_Hex (Unsigned_64 (To_Integer (E.Ptr)));
            Out_Str (" "); Out_Dec (E.Size); Out_Str ("B");
            Out_Char (ASCII.LF);
         end if;
      end loop;
   end Dump_Last;

end Audit_Log;
