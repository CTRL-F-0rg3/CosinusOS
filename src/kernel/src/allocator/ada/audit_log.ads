-- CosinusOS — allocator/ada/audit_log.ads

with System;
with Interfaces;   use Interfaces;
with Interfaces.C; use Interfaces.C;

package Audit_Log is

   AUDIT_SIZE : constant := 512;

   subtype C_Bool is Interfaces.C.int;

   procedure Log_Alloc (Ptr : System.Address; Size : Unsigned_64; Is_Slab : C_Bool)
     with Export, Convention => C, External_Name => "ada_audit_alloc";

   procedure Log_Free (Ptr : System.Address; Size : Unsigned_64; Is_Slab : C_Bool)
     with Export, Convention => C, External_Name => "ada_audit_free";

   procedure Get_Stats
     (Total_Allocs : out Unsigned_64;
      Total_Frees  : out Unsigned_64;
      Live_Bytes   : out Unsigned_64)
     with Export, Convention => C, External_Name => "ada_audit_stats";

   procedure Dump_Last (N : Unsigned_32)
     with Export, Convention => C, External_Name => "ada_audit_dump";

end Audit_Log;
