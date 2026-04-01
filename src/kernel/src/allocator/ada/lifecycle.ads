-- CosinusOS — allocator/ada/lifecycle.ads

with System;
with Interfaces;   use Interfaces;
with Interfaces.C;

package Lifecycle is

   procedure Init (Base : System.Address; Size : Unsigned_64)
     with Export, Convention => C, External_Name => "ada_alloc_init";

   procedure Reinit (Base : System.Address; Size : Unsigned_64)
     with Export, Convention => C, External_Name => "ada_alloc_reinit";

   procedure Shutdown
     with Export, Convention => C, External_Name => "ada_alloc_shutdown";

   function Version return Unsigned_32
     with Export, Convention => C, External_Name => "ada_alloc_version";

   function Is_Initialized return Interfaces.C.int
     with Export, Convention => C, External_Name => "ada_alloc_is_initialized";

   procedure Get_Heap_Range (Base : out System.Address; Size : out Unsigned_64)
     with Export, Convention => C, External_Name => "ada_alloc_heap_range";

end Lifecycle;
