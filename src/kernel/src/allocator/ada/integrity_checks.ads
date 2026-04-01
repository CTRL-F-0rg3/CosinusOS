-- CosinusOS — allocator/ada/integrity_checks.ads

with System;
with Interfaces;   use Interfaces;
with Interfaces.C; use Interfaces.C;

package Integrity_Checks is

   CANARY_VALUE   : constant Unsigned_64 := 16#DEADBEEFCAFEBABE#;
   PAGE_SIZE      : constant := 16#1000#;
   DFREE_BUF_SIZE : constant := 256;

   subtype C_Bool is Interfaces.C.int;
   C_False : constant C_Bool := 0;
   C_True  : constant C_Bool := 1;

   procedure Write_Canary (Ptr : System.Address; Size : Unsigned_64)
     with Export, Convention => C, External_Name => "ada_write_canary";

   function Check_Canary (Ptr : System.Address; Size : Unsigned_64)
     return C_Bool
     with Export, Convention => C, External_Name => "ada_check_canary";

   procedure Register_Free (Ptr : System.Address)
     with Export, Convention => C, External_Name => "ada_register_free";

   function Is_Double_Free (Ptr : System.Address) return C_Bool
     with Export, Convention => C, External_Name => "ada_is_double_free";

   function Check_Bounds
     (Ptr             : System.Address;
      Size            : Unsigned_64;
      Heap_Base       : System.Address;
      Heap_Size       : Unsigned_64;
      Need_Page_Align : C_Bool)
     return C_Bool
     with Export, Convention => C, External_Name => "ada_check_bounds";

end Integrity_Checks;
