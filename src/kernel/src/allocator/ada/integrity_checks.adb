-- CosinusOS — allocator/ada/integrity_checks.adb

with System.Machine_Code;     use System.Machine_Code;
with System.Storage_Elements; use System.Storage_Elements;

package body Integrity_Checks is
   use type System.Address;

   procedure Debug_Str (S : String) is
   begin
      for Ch of S loop
         Asm ("outb %0, $0xe9",
              Inputs => Character'Asm_Input ("a", Ch), Volatile => True);
      end loop;
      Asm ("outb %0, $0xe9",
           Inputs => Character'Asm_Input ("a", ASCII.LF), Volatile => True);
   end Debug_Str;

   -- -------------------------------------------------------------------------
   -- Canary
   -- -------------------------------------------------------------------------
   procedure Write_Canary (Ptr : System.Address; Size : Unsigned_64) is
      Slot : Unsigned_64
        with Address => Ptr + Storage_Offset (Size - 8), Import, Volatile;
   begin
      if Size < 8 then return; end if;
      Slot := CANARY_VALUE;
   end Write_Canary;

   function Check_Canary (Ptr : System.Address; Size : Unsigned_64) return C_Bool is
      Slot : Unsigned_64
        with Address => Ptr + Storage_Offset (Size - 8), Import, Volatile;
   begin
      if Size < 8 then return C_True; end if;
      if Slot /= CANARY_VALUE then
         Debug_Str ("[INTEGRITY] canary overwrite");
         return C_False;
      end if;
      return C_True;
   end Check_Canary;

   -- -------------------------------------------------------------------------
   -- Double-free ring buffer
   -- -------------------------------------------------------------------------
   type Addr_Array is array (0 .. DFREE_BUF_SIZE - 1) of System.Address;

   Free_Ring : Addr_Array;
   Ring_Head : Integer := 0;
   Ring_Init : Boolean := False;

   procedure Ensure_Init is
   begin
      if not Ring_Init then
         for I in Free_Ring'Range loop
            Free_Ring (I) := System.Null_Address;
         end loop;
         Ring_Init := True;
      end if;
   end Ensure_Init;

   procedure Register_Free (Ptr : System.Address) is
   begin
      Ensure_Init;
      Free_Ring (Ring_Head) := Ptr;
      Ring_Head := (Ring_Head + 1) mod DFREE_BUF_SIZE;
   end Register_Free;

   function Is_Double_Free (Ptr : System.Address) return C_Bool is
   begin
      Ensure_Init;
      for I in Free_Ring'Range loop
         if Free_Ring (I) = Ptr then
            Debug_Str ("[INTEGRITY] double free");
            return C_True;
         end if;
      end loop;
      return C_False;
   end Is_Double_Free;

   -- -------------------------------------------------------------------------
   -- Bounds check
   -- -------------------------------------------------------------------------
   function Check_Bounds
     (Ptr             : System.Address;
      Size            : Unsigned_64;
      Heap_Base       : System.Address;
      Heap_Size       : Unsigned_64;
      Need_Page_Align : C_Bool)
     return C_Bool
   is
      Heap_End  : constant System.Address :=
        Heap_Base + Storage_Offset (Heap_Size);
      Block_End : constant System.Address :=
        Ptr + Storage_Offset (Size);
      Ptr_Int   : constant Integer_Address := To_Integer (Ptr);
   begin
      if Ptr = System.Null_Address then
         Debug_Str ("[INTEGRITY] null ptr"); return C_False;
      end if;
      if Ptr < Heap_Base then
         Debug_Str ("[INTEGRITY] below heap"); return C_False;
      end if;
      if Block_End > Heap_End then
         Debug_Str ("[INTEGRITY] past heap end"); return C_False;
      end if;
      if Size = 0 then
         Debug_Str ("[INTEGRITY] zero size"); return C_False;
      end if;
      if Need_Page_Align /= C_False and then (Ptr_Int mod PAGE_SIZE) /= 0 then
         Debug_Str ("[INTEGRITY] not page-aligned"); return C_False;
      end if;
      return C_True;
   end Check_Bounds;

end Integrity_Checks;
