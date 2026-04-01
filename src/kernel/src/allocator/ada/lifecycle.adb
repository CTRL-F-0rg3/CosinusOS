-- CosinusOS — allocator/ada/lifecycle.adb

with System.Machine_Code;     use System.Machine_Code;
with System.Storage_Elements; use System.Storage_Elements;
with Interfaces.C;            use Interfaces.C;
with Audit_Log;

package body Lifecycle is
   use type System.Address;

   Initialized : Boolean        := False;
   Heap_Base   : System.Address := System.Null_Address;
   Heap_Sz     : Unsigned_64    := 0;
   Ver         : Unsigned_32    := 0;

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

   procedure Out_Hex16 (V : Unsigned_64) is
      Hex_Chars : constant String := "0123456789abcdef";
      Tmp       : Unsigned_64 := V;
      Buf       : String (1 .. 16);
   begin
      for I in reverse 1 .. 16 loop
         Buf (I) := Hex_Chars (Integer (Tmp and 16#F#) + 1);
         Tmp := Shift_Right (Tmp, 4);
      end loop;
      Out_Str (Buf);
   end Out_Hex16;

   -- -------------------------------------------------------------------------
   -- Public
   -- -------------------------------------------------------------------------
   procedure Init (Base : System.Address; Size : Unsigned_64) is
   begin
      Heap_Base   := Base;
      Heap_Sz     := Size;
      Ver         := Ver + 1;
      Initialized := True;
      Out_Str ("[LC] init base=0x");
      Out_Hex16 (Unsigned_64 (To_Integer (Base)));
      Out_Str (" size="); Out_Dec (Size);
      Out_Str ("B ver="); Out_Dec (Unsigned_64 (Ver));
      Out_Char (ASCII.LF);
   end Init;

   procedure Reinit (Base : System.Address; Size : Unsigned_64) is
      A, F, L : Unsigned_64;
   begin
      Audit_Log.Get_Stats (A, F, L);
      Out_Str ("[LC] reinit old: allocs="); Out_Dec (A);
      Out_Str (" frees="); Out_Dec (F);
      Out_Str (" live="); Out_Dec (L); Out_Str ("B"); Out_Char (ASCII.LF);
      Heap_Base   := Base;
      Heap_Sz     := Size;
      Ver         := Ver + 1;
      Initialized := True;
      Out_Str ("[LC] reinit base=0x");
      Out_Hex16 (Unsigned_64 (To_Integer (Base)));
      Out_Str (" size="); Out_Dec (Size);
      Out_Str ("B ver="); Out_Dec (Unsigned_64 (Ver));
      Out_Char (ASCII.LF);
   end Reinit;

   procedure Shutdown is
      A, F, L : Unsigned_64;
   begin
      Audit_Log.Get_Stats (A, F, L);
      Out_Str ("[LC] shutdown ver="); Out_Dec (Unsigned_64 (Ver));
      Out_Str (" allocs="); Out_Dec (A);
      Out_Str (" frees="); Out_Dec (F);
      Out_Str (" live="); Out_Dec (L); Out_Str ("B");
      Out_Char (ASCII.LF);
      if L /= 0 then
         Out_Str ("[LC] WARNING "); Out_Dec (L);
         Out_Str ("B leaked"); Out_Char (ASCII.LF);
      end if;
      Initialized := False;
   end Shutdown;

   function Version return Unsigned_32 is (Ver);

   function Is_Initialized return Interfaces.C.int is
   begin
      if Initialized then return 1; else return 0; end if;
   end Is_Initialized;

   procedure Get_Heap_Range (Base : out System.Address; Size : out Unsigned_64) is
   begin
      Base := Heap_Base;
      Size := Heap_Sz;
   end Get_Heap_Range;

end Lifecycle;
