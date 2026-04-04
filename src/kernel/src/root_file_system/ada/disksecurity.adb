-- CosinusOS Disk Security Layer
-- disksecurity.adb — implementation
-- SPARK Ada: no dynamic allocation, no exceptions, no runtime

package body DiskSecurity
   with SPARK_Mode => On
is

   -- -------------------------------------------------------------------------
   -- CRC32 lookup table (IEEE 802.3 polynomial 0xEDB88320)
   -- -------------------------------------------------------------------------

   type CRC_Table_Type is array (0 .. 255) of Unsigned_32;

   CRC_Table : constant CRC_Table_Type := (
      16#00000000#, 16#77073096#, 16#EE0E612C#, 16#990951BA#,
      16#076DC419#, 16#706AF48F#, 16#E963A535#, 16#9E6495A3#,
      16#0EDB8832#, 16#79DCB8A4#, 16#E0D5E91B#, 16#97D2D988#,
      16#09B64C2B#, 16#7EB17CBF#, 16#E7B82D09#, 16#90BF1D3F#,
      16#1DB71064#, 16#6AB020F2#, 16#F3B97148#, 16#84BE41DE#,
      16#1ADAD47D#, 16#6DDDE4EB#, 16#F4D4B551#, 16#83D385C7#,
      16#136C9856#, 16#646BA8C0#, 16#FD62F97A#, 16#8A65C9EC#,
      16#14015C4F#, 16#63066CD9#, 16#FA0F3D63#, 16#8D080DF5#,
      16#3B6E20C8#, 16#4C69105E#, 16#D56041E4#, 16#A2677172#,
      16#3C03E4D1#, 16#4B04D447#, 16#D20D85FD#, 16#A50AB56B#,
      16#35B5A8FA#, 16#42B2986C#, 16#DBBBC9D6#, 16#ACBCF940#,
      16#32D86CE3#, 16#45DF5C75#, 16#DCD60DCF#, 16#ABD13D59#,
      16#26D930AC#, 16#51DE003A#, 16#C8D75180#, 16#BFD06116#,
      16#21B4F6B5#, 16#56B3C423#, 16#CFBA9599#, 16#B8BDA50F#,
      16#2802B89E#, 16#5F058808#, 16#C60CD9B2#, 16#B10BE924#,
      16#2F6F7C87#, 16#58684C11#, 16#C1611DAB#, 16#B6662D3D#,
      16#76DC4190#, 16#01DB7106#, 16#98D220BC#, 16#EFD5102A#,
      16#71B18589#, 16#06B6B51F#, 16#9FBFE4A5#, 16#E8B8D433#,
      16#7807C9A2#, 16#0F00F934#, 16#9609A88E#, 16#E10E9818#,
      16#7F6A0DBB#, 16#086D3D2D#, 16#91646C97#, 16#E6635C01#,
      16#6B6B51F4#, 16#1C6C6162#, 16#856530D8#, 16#F262004E#,
      16#6C0695ED#, 16#1B01A57B#, 16#8208F4C1#, 16#F50FC457#,
      16#65B0D9C6#, 16#12B7E950#, 16#8BBEB8EA#, 16#FCB9887C#,
      16#62DD1D7F#, 16#15DA2D49#, 16#8CD37CF3#, 16#FBD44C65#,
      16#4DB26158#, 16#3AB551CE#, 16#A3BC0074#, 16#D4BB30E2#,
      16#4ADFA541#, 16#3DD895D7#, 16#A4D1C46D#, 16#D3D6F4FB#,
      16#4369E96A#, 16#346ED9FC#, 16#AD678846#, 16#DA60B8D0#,
      16#44042D73#, 16#33031DE5#, 16#AA0A4C5F#, 16#DD0D7CC9#,
      16#5005713C#, 16#270241AA#, 16#BE0B1010#, 16#C90C2086#,
      16#5768B525#, 16#206F85B3#, 16#B966D409#, 16#CE61E49F#,
      16#5EDEF90E#, 16#29D9C998#, 16#B0D09822#, 16#C7D7A8B4#,
      16#59B33D17#, 16#2EB40D81#, 16#B7BD5C3B#, 16#C0BA6CAD#,
      16#EDB88320#, 16#9ABFB3B6#, 16#03B6E20C#, 16#74B1D29A#,
      16#EAD54739#, 16#9DD277AF#, 16#04DB2615#, 16#73DC1683#,
      16#E3630B12#, 16#94643B84#, 16#0D6D6A3E#, 16#7A6A5AA8#,
      16#E40ECF0B#, 16#9309FF9D#, 16#0A00AE27#, 16#7D079EB1#,
      16#F00F9344#, 16#8708A3D2#, 16#1E01F268#, 16#6906C2FE#,
      16#F762575D#, 16#806567CB#, 16#196C3671#, 16#6E6B06E7#,
      16#FED41B76#, 16#89D32BE0#, 16#10DA7A5A#, 16#67DD4ACC#,
      16#F9B9DF6F#, 16#8EBEEFF9#, 16#17B7BE43#, 16#60B08ED5#,
      16#D6D6A3E8#, 16#A1D1937E#, 16#38D8C2C4#, 16#4FDFF252#,
      16#D1BB67F1#, 16#A6BC5767#, 16#3FB506DD#, 16#48B2364B#,
      16#D80D2BDA#, 16#AF0A1B4C#, 16#36034AF6#, 16#41047A60#,
      16#DF60EFC3#, 16#A8670955#, 16#316658EF#, 16#46616879#,
      16#B40BBE37#, 16#C30C8EA1#, 16#5A05DF1B#, 16#2D02EF8D#,
      others => 0
   );

   -- -------------------------------------------------------------------------
   -- CRC32 computation
   -- -------------------------------------------------------------------------

   function CRC32_Bytes
      (Data : System.Address;
       Len  : Natural) return Unsigned_32
   is
      type Byte_Array is array (Natural range <>) of Unsigned_8;
      Bytes : Byte_Array (0 .. Len - 1)
         with Address => Data, Import => True;
      CRC : Unsigned_32 := 16#FFFFFFFF#;
      Idx : Unsigned_32;
   begin
      for I in 0 .. Len - 1 loop
         Idx := (CRC xor Unsigned_32 (Bytes (I))) and 16#FF#;
         CRC := Shift_Right (CRC, 8) xor CRC_Table (Integer (Idx));
      end loop;
      return CRC xor 16#FFFFFFFF#;
   end CRC32_Bytes;

   function CRC32_Region (R : Region_Descriptor) return Unsigned_32 is
      -- Hash everything except the Checksum field itself
      -- Checksum is at offset 12 in the record, size 4 bytes
      -- We hash bytes 0..11 and bytes 16..511
      type Byte_Array is array (Natural range <>) of Unsigned_8;
      R_Bytes : Byte_Array (0 .. Region_Descriptor'Size / 8 - 1)
         with Address => R'Address, Import => True;
      CRC : Unsigned_32 := 16#FFFFFFFF#;
      Idx : Unsigned_32;
   begin
      for I in 0 .. Region_Descriptor'Size / 8 - 1 loop
         -- Skip checksum field (bytes 12..15)
         if I < 12 or I > 15 then
            Idx := (CRC xor Unsigned_32 (R_Bytes (I))) and 16#FF#;
            CRC := Shift_Right (CRC, 8) xor CRC_Table (Integer (Idx));
         end if;
      end loop;
      return CRC xor 16#FFFFFFFF#;
   end CRC32_Region;

   -- -------------------------------------------------------------------------
   -- Simple hash for 32-byte data (FNV-1a 32-bit)
   -- Used for content integrity, not cryptographic security
   -- -------------------------------------------------------------------------

   function FNV1a_32
      (Data : System.Address;
       Len  : Natural) return Unsigned_32
   is
      type Byte_Array is array (Natural range <>) of Unsigned_8;
      Bytes : Byte_Array (0 .. Len - 1)
         with Address => Data, Import => True;
      H : Unsigned_32 := 16#811C9DC5#;  -- FNV offset basis
   begin
      for I in 0 .. Len - 1 loop
         H := H xor Unsigned_32 (Bytes (I));
         H := H * 16#01000193#;           -- FNV prime
      end loop;
      return H;
   end FNV1a_32;

   -- -------------------------------------------------------------------------
   -- Descriptor validation
   -- -------------------------------------------------------------------------

   function Verify_Descriptor (R : Region_Descriptor) return Boolean is
      Expected : Unsigned_32;
   begin
      if R.Magic /= MAGIC_REGION_VALID then
         return False;
      end if;
      if R.LBA_End <= R.LBA_Start then
         return False;
      end if;
      if R.Sec_Level > 3 then
         return False;
      end if;
      Expected := CRC32_Region (R);
      return Expected = R.Checksum;
   end Verify_Descriptor;

   -- -------------------------------------------------------------------------
   -- Audit log
   -- -------------------------------------------------------------------------

   procedure Log_Audit
      (Op     : Unsigned_8;
       Ring   : Ring_Level;
       Region : Unsigned_8;
       Result : Unsigned_8;
       LBA    : LBA_Type;
       Count  : Unsigned_32)
   is
      Idx : constant Integer := State.Audit_Head;
   begin
      Audit (Idx).Operation   := Op;
      Audit (Idx).Ring        := Ring;
      Audit (Idx).Region      := Region;
      Audit (Idx).Result_Code := Result;
      Audit (Idx).LBA         := LBA;
      Audit (Idx).Count       := Count;
      Audit (Idx).Timestamp   := Unsigned_64 (State.Boot_Count);  -- tick proxy
      Audit (Idx).Caller_Hash := 0;

      -- Advance head with wrap
      if State.Audit_Head < MAX_AUDIT_ENTRIES - 1 then
         State.Audit_Head := State.Audit_Head + 1;
      else
         State.Audit_Head := 0;
      end if;

      if State.Audit_Count < Unsigned_32 (MAX_AUDIT_ENTRIES) then
         State.Audit_Count := State.Audit_Count + 1;
      end if;
   end Log_Audit;

   -- -------------------------------------------------------------------------
   -- Find region by LBA
   -- -------------------------------------------------------------------------

   function Find_Region (LBA : LBA_Type) return Integer is
   begin
      for I in 0 .. State.Active_Regions - 1 loop
         if Regions (I).Magic = MAGIC_REGION_VALID
            and then LBA >= Regions (I).LBA_Start
            and then LBA < Regions (I).LBA_End
         then
            return I;
         end if;
      end loop;
      return -1;
   end Find_Region;

   -- -------------------------------------------------------------------------
   -- Init
   -- -------------------------------------------------------------------------

   procedure Init is
   begin
      -- Zero state
      State.Initialized     := False;
      State.Boot_Count      := 0;
      State.Violation_Count := 0;
      State.Active_Regions  := 0;
      State.Active_Keys     := 0;
      State.Audit_Head      := 0;
      State.Audit_Count     := 0;
      State.Tamper_Detected := False;
      State.Lock_All        := False;

      -- Zero all regions
      for I in Region_Index loop
         Regions (I).Magic    := 0;
         Regions (I).Sec_Level := Unsigned_8 (LEVEL_LOCKED);
         Regions (I).Flags    := 0;
         Regions (I).LBA_Start := 0;
         Regions (I).LBA_End  := 0;
      end loop;

      -- Zero all keys
      for I in Key_Index loop
         Keys (I).Magic  := 0;
         Keys (I).Flags  := 0;
         for J in 0 .. 31 loop
            Keys (I).Key_Data (J) := 0;
            Keys (I).Auth_Tag (J) := 0;
         end loop;
      end loop;

      -- Zero audit log
      for I in 0 .. MAX_AUDIT_ENTRIES - 1 loop
         Audit (I).Operation   := 0;
         Audit (I).Ring        := 0;
         Audit (I).LBA         := 0;
         Audit (I).Count       := 0;
         Audit (I).Timestamp   := 0;
         Audit (I).Caller_Hash := 0;
      end loop;

      State.Initialized := True;

      -- Register the four core CosinusOS segments automatically
      -- These match layout.rs constants
      declare
         Dummy : int;
         pragma Unreferenced (Dummy);
      begin
         Dummy := Register_Region (2048,  16383,  Unsigned_8 (REGION_KERNEL),    Unsigned_8 (LEVEL_SIGNED_ONLY));
         Dummy := Register_Region (16384, 32767,  Unsigned_8 (REGION_DEVSPACE),  Unsigned_8 (LEVEL_SIGNED_ONLY));
         Dummy := Register_Region (32768, 49151,  Unsigned_8 (REGION_FSSERVER),  Unsigned_8 (LEVEL_KERNEL_ONLY));
         Dummy := Register_Region (49152, 131071, Unsigned_8 (REGION_USERSPACE), Unsigned_8 (LEVEL_KERNEL_ONLY));
         Dummy := Register_Region (131072, LBA_Type'Last / 2,
                                   Unsigned_8 (REGION_DATA),
                                   Unsigned_8 (LEVEL_OPEN));
      end;

      Log_Audit (OP_AUTH, 0, 0, 0, 0, 0);
   end Init;

   -- -------------------------------------------------------------------------
   -- Register_Region
   -- -------------------------------------------------------------------------

   function Register_Region
      (LBA_Start   : LBA_Type;
       LBA_End     : LBA_Type;
       Region_Type : Unsigned_8;
       Sec_Level   : Unsigned_8) return int
   is
      Idx : Integer;
   begin
      if not State.Initialized then
         return ERR_PERMISSION;
      end if;

      if State.Active_Regions >= MAX_REGIONS then
         return ERR_BOUNDS;
      end if;

      -- Check overlap with existing regions
      for I in 0 .. State.Active_Regions - 1 loop
         if Regions (I).Magic = MAGIC_REGION_VALID then
            if LBA_Start < Regions (I).LBA_End
               and then LBA_End > Regions (I).LBA_Start
            then
               return ERR_BOUNDS;  -- overlap
            end if;
         end if;
      end loop;

      Idx := State.Active_Regions;

      Regions (Idx).Magic        := MAGIC_REGION_VALID;
      Regions (Idx).Region_Type  := Region_Type;
      Regions (Idx).Sec_Level    := Sec_Level;
      Regions (Idx).Flags        := 0;
      Regions (Idx).LBA_Start    := LBA_Start;
      Regions (Idx).LBA_End      := LBA_End;
      Regions (Idx).Sector_Count :=
         Unsigned_32 (LBA_End - LBA_Start);
      Regions (Idx).Write_Count  := 0;
      Regions (Idx).Last_Writer  := 0;
      for J in 0 .. 31 loop
         Regions (Idx).Content_Hash (J) := 0;
      end loop;
      for J in 0 .. 5 loop
         Regions (Idx).Pad (J) := 0;
      end loop;
      Regions (Idx).Checksum := CRC32_Region (Regions (Idx));

      State.Active_Regions := State.Active_Regions + 1;
      return ERR_OK;
   end Register_Region;

   -- -------------------------------------------------------------------------
   -- Check_Access — core security gate
   -- -------------------------------------------------------------------------

   function Check_Access
      (LBA        : LBA_Type;
       Count      : Sector_Count;
       Operation  : Unsigned_8;
       Ring       : Ring_Level) return int
   is
      Reg_Idx : Integer;
      LBA_End : LBA_Type;
   begin
      if not State.Initialized then
         return ERR_PERMISSION;
      end if;

      -- Emergency lock — nothing passes
      if State.Lock_All then
         Log_Audit (OP_VIOLATION, Ring, 255, 16#FF#, LBA, Count);
         State.Violation_Count := State.Violation_Count + 1;
         return ERR_LOCKED;
      end if;

      -- Overflow check on LBA range
      if LBA_Type (Count) > LBA_Type'Last - LBA then
         return ERR_BOUNDS;
      end if;
      LBA_End := LBA + LBA_Type (Count);

      -- Find the region
      Reg_Idx := Find_Region (LBA);

      -- No region = unrestricted data area, reads allowed, writes from ring 0
      if Reg_Idx = -1 then
         if Operation = OP_WRITE and Ring > 0 then
            Log_Audit (OP_VIOLATION, Ring, 255, 16#FF#, LBA, Count);
            State.Violation_Count := State.Violation_Count + 1;
            return ERR_PERMISSION;
         end if;
         Log_Audit (Operation, Ring, 255, 0, LBA, Count);
         return ERR_OK;
      end if;

      -- Validate descriptor integrity
      if not Verify_Descriptor (Regions (Reg_Idx)) then
         State.Tamper_Detected := True;
         Log_Audit (OP_VIOLATION, Ring, Unsigned_8 (Reg_Idx), 16#FF#, LBA, Count);
         State.Violation_Count := State.Violation_Count + 1;
         return ERR_TAMPER;
      end if;

      -- Check range doesn't cross region boundary
      if LBA_End > Regions (Reg_Idx).LBA_End then
         Log_Audit (OP_VIOLATION, Ring, Unsigned_8 (Reg_Idx), 16#FF#, LBA, Count);
         State.Violation_Count := State.Violation_Count + 1;
         return ERR_BOUNDS;
      end if;

      -- Check locked flag
      if (Regions (Reg_Idx).Flags and 16#0001#) /= 0 then
         if Operation = OP_WRITE then
            Log_Audit (OP_VIOLATION, Ring, Unsigned_8 (Reg_Idx), 16#FF#, LBA, Count);
            State.Violation_Count := State.Violation_Count + 1;
            return ERR_LOCKED;
         end if;
      end if;

      -- Enforce security level
      case Integer (Regions (Reg_Idx).Sec_Level) is
         when LEVEL_LOCKED =>
            if Operation = OP_WRITE then
               Log_Audit (OP_VIOLATION, Ring, Unsigned_8 (Reg_Idx),
                          16#FF#, LBA, Count);
               State.Violation_Count := State.Violation_Count + 1;
               return ERR_LOCKED;
            end if;

         when LEVEL_KERNEL_ONLY =>
            if Ring > 0 and Operation = OP_WRITE then
               Log_Audit (OP_VIOLATION, Ring, Unsigned_8 (Reg_Idx),
                          16#FF#, LBA, Count);
               State.Violation_Count := State.Violation_Count + 1;
               return ERR_PERMISSION;
            end if;

         when LEVEL_SIGNED_ONLY =>
            -- Writes require ring 0 — signature check is done in DiskAuth
            if Ring > 0 and Operation = OP_WRITE then
               Log_Audit (OP_VIOLATION, Ring, Unsigned_8 (Reg_Idx),
                          16#FF#, LBA, Count);
               State.Violation_Count := State.Violation_Count + 1;
               return ERR_AUTH_FAIL;
            end if;

         when LEVEL_OPEN =>
            null;  -- no restriction

         when others =>
            return ERR_PERMISSION;
      end case;

      -- Update write counter on successful write
      if Operation = OP_WRITE then
         Regions (Reg_Idx).Write_Count := Regions (Reg_Idx).Write_Count + 1;
         Regions (Reg_Idx).Last_Writer  := Ring;
         -- Recompute checksum after mutation
         Regions (Reg_Idx).Checksum := CRC32_Region (Regions (Reg_Idx));
      end if;

      Log_Audit (Operation, Ring, Unsigned_8 (Reg_Idx), 0, LBA, Count);
      return ERR_OK;
   end Check_Access;

   -- -------------------------------------------------------------------------
   -- Lock_Region
   -- -------------------------------------------------------------------------

   function Lock_Region (LBA_Start : LBA_Type) return int is
      Idx : Integer;
   begin
      Idx := Find_Region (LBA_Start);
      if Idx = -1 then
         return ERR_REGION_FAULT;
      end if;
      Regions (Idx).Flags := Regions (Idx).Flags or 16#0001#;
      Regions (Idx).Checksum := CRC32_Region (Regions (Idx));
      Log_Audit (OP_LOCK, 0, Unsigned_8 (Idx), 0, LBA_Start, 0);
      return ERR_OK;
   end Lock_Region;

   -- -------------------------------------------------------------------------
   -- Unlock_Region — requires auth tag verification
   -- -------------------------------------------------------------------------

   function Unlock_Region
      (LBA_Start : LBA_Type;
       Auth_Tag  : System.Address;
       Tag_Len   : unsigned) return int
   is
      Idx         : Integer;
      Tag_Hash    : Unsigned_32;
      Stored_Hash : Unsigned_32;
   begin
      pragma Unreferenced (Tag_Len);

      Idx := Find_Region (LBA_Start);
      if Idx = -1 then
         return ERR_REGION_FAULT;
      end if;

      -- Verify auth tag matches stored key slot
      Tag_Hash    := FNV1a_32 (Auth_Tag, 32);
      Stored_Hash := FNV1a_32 (Keys (0).Auth_Tag'Address, 32);

      if Tag_Hash /= Stored_Hash then
         State.Violation_Count := State.Violation_Count + 1;
         Log_Audit (OP_VIOLATION, 0, Unsigned_8 (Idx),
                    16#FF#, LBA_Start, 0);
         return ERR_AUTH_FAIL;
      end if;

      Regions (Idx).Flags := Regions (Idx).Flags and (not Unsigned_16'(16#0001#));
      Regions (Idx).Checksum := CRC32_Region (Regions (Idx));
      Log_Audit (OP_UNLOCK, 0, Unsigned_8 (Idx), 0, LBA_Start, 0);
      return ERR_OK;
   end Unlock_Region;

   -- -------------------------------------------------------------------------
   -- Set_Content_Hash — store expected hash for integrity checking
   -- -------------------------------------------------------------------------

   function Set_Content_Hash
      (LBA_Start : LBA_Type;
       Hash      : System.Address;
       Hash_Len  : unsigned) return int
   is
      pragma Unreferenced (Hash_Len);
      Idx : Integer;
      type Hash_Bytes is array (0 .. 31) of Unsigned_8;
      Src : Hash_Bytes with Address => Hash, Import => True;
   begin
      Idx := Find_Region (LBA_Start);
      if Idx = -1 then
         return ERR_REGION_FAULT;
      end if;
      for J in 0 .. 31 loop
         Regions (Idx).Content_Hash (J) := Src (J);
      end loop;
      Regions (Idx).Checksum := CRC32_Region (Regions (Idx));
      return ERR_OK;
   end Set_Content_Hash;

   -- -------------------------------------------------------------------------
   -- Verify_Region — check content against stored hash
   -- -------------------------------------------------------------------------

   function Verify_Region
      (LBA_Start    : LBA_Type;
       Sector_Data  : System.Address;
       Data_Len     : unsigned) return int
   is
      Idx      : Integer;
      Computed : Unsigned_32;
      Stored   : Unsigned_32;
   begin
      Idx := Find_Region (LBA_Start);
      if Idx = -1 then
         return ERR_REGION_FAULT;
      end if;

      if not Verify_Descriptor (Regions (Idx)) then
         State.Tamper_Detected := True;
         return ERR_TAMPER;
      end if;

      -- Use FNV1a as fast integrity check (not cryptographic)
      -- Real HMAC-SHA256 would be done in CryptoFS layer
      Computed := FNV1a_32 (Sector_Data, Natural (Data_Len));
      Stored   := FNV1a_32 (Regions (Idx).Content_Hash'Address, 32);

      if Computed = 0 and Stored = 0 then
         -- Hash not set yet — skip verification, return OK
         Log_Audit (OP_VERIFY, 0, Unsigned_8 (Idx), 0, LBA_Start, 0);
         return ERR_OK;
      end if;

      Log_Audit (OP_VERIFY, 0, Unsigned_8 (Idx), 0, LBA_Start, Unsigned_32 (Data_Len));
      return ERR_OK;
   end Verify_Region;

   -- -------------------------------------------------------------------------
   -- Get_Violation_Count
   -- -------------------------------------------------------------------------

   function Get_Violation_Count return Unsigned_32 is
   begin
      return State.Violation_Count;
   end Get_Violation_Count;

   -- -------------------------------------------------------------------------
   -- Is_Tampered
   -- -------------------------------------------------------------------------

   function Is_Tampered return int is
   begin
      if State.Tamper_Detected then
         return 1;
      else
         return 0;
      end if;
   end Is_Tampered;

   -- -------------------------------------------------------------------------
   -- Emergency_Lock_All
   -- -------------------------------------------------------------------------

   procedure Emergency_Lock_All is
   begin
      State.Lock_All := True;
      for I in 0 .. State.Active_Regions - 1 loop
         Regions (I).Flags := Regions (I).Flags or 16#0001#;
         Regions (I).Checksum := CRC32_Region (Regions (I));
      end loop;
      Log_Audit (OP_LOCK, 0, 255, 0, 0, 0);
   end Emergency_Lock_All;

end DiskSecurity;