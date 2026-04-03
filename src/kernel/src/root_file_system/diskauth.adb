-- CosinusOS Disk Auth Layer
-- diskauth.adb — implementation

package body DiskAuth
   with SPARK_Mode => On
is

   -- -------------------------------------------------------------------------
   -- CRC32 (same polynomial as DiskSecurity)
   -- -------------------------------------------------------------------------

   type CRC_Table_T is array (0 .. 255) of Unsigned_32;

   CRC32_Tab : constant CRC_Table_T := (
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

   function CRC32_Bytes_Auth (Data : System.Address; Len : Natural) return Unsigned_32 is
      type BA is array (Natural range <>) of Unsigned_8;
      B   : BA (0 .. Len - 1) with Address => Data, Import => True;
      CRC : Unsigned_32 := 16#FFFFFFFF#;
      Idx : Unsigned_32;
   begin
      for I in 0 .. Len - 1 loop
         Idx := (CRC xor Unsigned_32 (B (I))) and 16#FF#;
         CRC := Shift_Right (CRC, 8) xor CRC32_Tab (Integer (Idx));
      end loop;
      return CRC xor 16#FFFFFFFF#;
   end CRC32_Bytes_Auth;

   function CRC32_Permit (P : Write_Permit) return Unsigned_32 is
      type BA is array (Natural range <>) of Unsigned_8;
      PB : BA (0 .. Write_Permit'Size / 8 - 1)
         with Address => P'Address, Import => True;
      CRC : Unsigned_32 := 16#FFFFFFFF#;
      Idx : Unsigned_32;
      CS_Off : constant Natural := 96;  -- offset of Checksum field in bytes
   begin
      for I in 0 .. Write_Permit'Size / 8 - 1 loop
         if I < CS_Off or I > CS_Off + 3 then
            Idx := (CRC xor Unsigned_32 (PB (I))) and 16#FF#;
            CRC := Shift_Right (CRC, 8) xor CRC32_Tab (Integer (Idx));
         end if;
      end loop;
      return CRC xor 16#FFFFFFFF#;
   end CRC32_Permit;

   -- -------------------------------------------------------------------------
   -- Token comparison — constant time
   -- -------------------------------------------------------------------------

   function Token_Match (A : Token_Bytes; B : Token_Bytes) return Boolean is
      Diff : Unsigned_8 := 0;
   begin
      for I in 0 .. TOKEN_SIZE - 1 loop
         Diff := Diff or (A (I) xor B (I));
      end loop;
      return Diff = 0;
   end Token_Match;

   -- -------------------------------------------------------------------------
   -- Session validity check
   -- -------------------------------------------------------------------------

   function Session_Valid (S : Auth_Session) return Boolean is
   begin
      if S.Magic /= SESSION_MAGIC then
         return False;
      end if;
      if S.State /= SESSION_ACTIVE then
         return False;
      end if;
      if State.Tick - S.Tick_Last > SESSION_TIMEOUT then
         return False;
      end if;
      return True;
   end Session_Valid;

   -- -------------------------------------------------------------------------
   -- Finders
   -- -------------------------------------------------------------------------

   function Find_Session (Id : Session_Id) return Integer is
   begin
      for I in 0 .. State.Session_Count - 1 loop
         if Sessions (I).Id = Id
            and Sessions (I).Magic = SESSION_MAGIC
         then
            return I;
         end if;
      end loop;
      return -1;
   end Find_Session;

   function Find_Free_Session return Integer is
   begin
      for I in 0 .. MAX_SESSIONS - 1 loop
         if Sessions (I).Magic /= SESSION_MAGIC
            or Sessions (I).State = SESSION_INVALID
            or Sessions (I).State = SESSION_EXPIRED
            or Sessions (I).State = SESSION_REVOKED
         then
            return I;
         end if;
      end loop;
      return -1;
   end Find_Free_Session;

   function Find_Free_Permit return Integer is
   begin
      for I in 0 .. MAX_PERMITS - 1 loop
         if Permits (I).Magic /= PERMIT_MAGIC
            or Permits (I).Used
         then
            return I;
         end if;
      end loop;
      return -1;
   end Find_Free_Permit;

   -- -------------------------------------------------------------------------
   -- Init
   -- -------------------------------------------------------------------------

   procedure Init is
   begin
      State.Initialized   := False;
      State.Tick          := 0;
      State.Session_Count := 0;
      State.Permit_Count  := 0;
      State.Total_Denials := 0;
      State.Total_Permits := 0;

      for I in 0 .. MAX_SESSIONS - 1 loop
         Sessions (I).Magic        := 0;
         Sessions (I).Id           := 0;
         Sessions (I).State        := SESSION_INVALID;
         Sessions (I).Ring         := 0;
         Sessions (I).Tick_Created := 0;
         Sessions (I).Tick_Last    := 0;
         Sessions (I).Permit_Count := 0;
         Sessions (I).Write_Count  := 0;
         for J in 0 .. TOKEN_SIZE - 1 loop
            Sessions (I).Token (J) := 0;
         end loop;
      end loop;

      for I in 0 .. MAX_PERMITS - 1 loop
         Permits (I).Magic       := 0;
         Permits (I).Session     := 0;
         Permits (I).LBA_Start   := 0;
         Permits (I).LBA_End     := 0;
         Permits (I).Flags       := 0;
         Permits (I).Ring        := 0;
         Permits (I).Used        := False;
         Permits (I).Tick_Issued := 0;
         Permits (I).Tick_Expiry := 0;
         Permits (I).Checksum    := 0;
         for J in 0 .. TOKEN_SIZE - 1 loop
            Permits (I).Token (J) := 0;
         end loop;
      end loop;

      State.Initialized := True;
   end Init;

   -- -------------------------------------------------------------------------
   -- Tick — advance clock and expire sessions
   -- -------------------------------------------------------------------------

   procedure Tick is
   begin
      if not State.Initialized then
         return;
      end if;

      State.Tick := State.Tick + 1;

      -- Expire stale sessions
      for I in 0 .. MAX_SESSIONS - 1 loop
         if Sessions (I).Magic = SESSION_MAGIC
            and Sessions (I).State = SESSION_ACTIVE
         then
            if State.Tick - Sessions (I).Tick_Last > SESSION_TIMEOUT then
               Sessions (I).State := SESSION_EXPIRED;
               if State.Session_Count > 0 then
                  State.Session_Count := State.Session_Count - 1;
               end if;
            end if;
         end if;
      end loop;
   end Tick;

   -- -------------------------------------------------------------------------
   -- Open_Session
   -- -------------------------------------------------------------------------

   function Open_Session
      (Ring      : Ring_Level;
       Token     : System.Address;
       Token_Len : unsigned) return int
   is
      pragma Unreferenced (Token_Len);
      Idx     : Integer;
      New_Id  : Session_Id;
      type TB is array (0 .. TOKEN_SIZE - 1) of Unsigned_8;
      Tok : TB with Address => Token, Import => True;
   begin
      if not State.Initialized then
         return ERR_NO_SESSION;
      end if;

      Idx := Find_Free_Session;
      if Idx = -1 then
         return ERR_FULL;
      end if;

      -- Generate session ID from tick + token hash
      New_Id := Session_Id (State.Tick and 16#FFFF_FFFF#)
                xor Session_Id (CRC32_Bytes_Auth (Token, TOKEN_SIZE));

      Sessions (Idx).Magic        := SESSION_MAGIC;
      Sessions (Idx).Id           := New_Id;
      Sessions (Idx).State        := SESSION_ACTIVE;
      Sessions (Idx).Ring         := Ring;
      Sessions (Idx).Tick_Created := State.Tick;
      Sessions (Idx).Tick_Last    := State.Tick;
      Sessions (Idx).Permit_Count := 0;
      Sessions (Idx).Write_Count  := 0;
      for J in 0 .. TOKEN_SIZE - 1 loop
         Sessions (Idx).Token (J) := Tok (J);
      end loop;

      if State.Session_Count < MAX_SESSIONS then
         State.Session_Count := State.Session_Count + 1;
      end if;

      return int (New_Id);
   end Open_Session;

   -- -------------------------------------------------------------------------
   -- Close_Session
   -- -------------------------------------------------------------------------

   function Close_Session (Id : Session_Id) return int is
      Idx : Integer;
   begin
      Idx := Find_Session (Id);
      if Idx = -1 then
         return ERR_NO_SESSION;
      end if;

      Sessions (Idx).State := SESSION_REVOKED;
      Sessions (Idx).Magic := 0;
      if State.Session_Count > 0 then
         State.Session_Count := State.Session_Count - 1;
      end if;

      -- Revoke all associated permits
      for I in 0 .. MAX_PERMITS - 1 loop
         if Permits (I).Magic = PERMIT_MAGIC
            and Permits (I).Session = Id
         then
            Permits (I).Magic := 0;
            Permits (I).Used  := True;
         end if;
      end loop;

      return ERR_OK;
   end Close_Session;

   -- -------------------------------------------------------------------------
   -- Issue_Permit
   -- -------------------------------------------------------------------------

   function Issue_Permit
      (Session   : Session_Id;
       LBA_Start : LBA_Type;
       LBA_End   : LBA_Type;
       Flags     : Unsigned_16) return int
   is
      Sidx : Integer;
      Pidx : Integer;
   begin
      Sidx := Find_Session (Session);
      if Sidx = -1 then
         return ERR_NO_SESSION;
      end if;

      if not Session_Valid (Sessions (Sidx)) then
         return ERR_EXPIRED;
      end if;

      -- Ring 3 cannot issue admin permits
      if (Flags and PERMIT_ADMIN) /= 0 and Sessions (Sidx).Ring > 0 then
         State.Total_Denials := State.Total_Denials + 1;
         return ERR_DENIED;
      end if;

      Pidx := Find_Free_Permit;
      if Pidx = -1 then
         return ERR_FULL;
      end if;

      Permits (Pidx).Magic       := PERMIT_MAGIC;
      Permits (Pidx).Session     := Session;
      Permits (Pidx).LBA_Start   := LBA_Start;
      Permits (Pidx).LBA_End     := LBA_End;
      Permits (Pidx).Flags       := Flags;
      Permits (Pidx).Ring        := Sessions (Sidx).Ring;
      Permits (Pidx).Used        := False;
      Permits (Pidx).Tick_Issued := State.Tick;
      Permits (Pidx).Tick_Expiry := State.Tick + SESSION_TIMEOUT;
      -- Copy session token into permit for binding
      for J in 0 .. TOKEN_SIZE - 1 loop
         Permits (Pidx).Token (J) := Sessions (Sidx).Token (J);
      end loop;
      Permits (Pidx).Checksum := CRC32_Permit (Permits (Pidx));

      if State.Permit_Count < MAX_PERMITS then
         State.Permit_Count := State.Permit_Count + 1;
      end if;
      State.Total_Permits := State.Total_Permits + 1;

      -- Update session last-used tick
      Sessions (Sidx).Tick_Last    := State.Tick;
      Sessions (Sidx).Permit_Count := Sessions (Sidx).Permit_Count + 1;

      return ERR_OK;
   end Issue_Permit;

   -- -------------------------------------------------------------------------
   -- Check_Permit
   -- -------------------------------------------------------------------------

   function Check_Permit
      (LBA_Start : LBA_Type;
       Count     : Unsigned_32;
       Ring      : Ring_Level;
       Flags     : Unsigned_16) return int
   is
      LBA_End : LBA_Type;
   begin
      -- Ring 0 with admin flag bypasses permit check
      if Ring = 0 and (Flags and PERMIT_ADMIN) /= 0 then
         return ERR_OK;
      end if;

      LBA_End := LBA_Start + LBA_Type (Count);

      for I in 0 .. MAX_PERMITS - 1 loop
         if Permits (I).Magic = PERMIT_MAGIC and not Permits (I).Used then
            -- Check expiry
            if State.Tick > Permits (I).Tick_Expiry then
               Permits (I).Used := True;  -- mark expired
            else
               -- Check LBA range overlap
               if LBA_Start >= Permits (I).LBA_Start
                  and LBA_End <= Permits (I).LBA_End
               then
                  -- Check flags
                  if (Permits (I).Flags and Flags) = Flags then
                     -- Check ring — permit ring must be >= caller ring
                     if Ring <= Permits (I).Ring then
                        -- Verify checksum
                        if CRC32_Permit (Permits (I)) = Permits (I).Checksum then
                           return ERR_OK;
                        end if;
                     end if;
                  end if;
               end if;
            end if;
         end if;
      end loop;

      State.Total_Denials := State.Total_Denials + 1;
      return ERR_NO_PERMIT;
   end Check_Permit;

   -- -------------------------------------------------------------------------
   -- Revoke_Session_Permits
   -- -------------------------------------------------------------------------

   function Revoke_Session_Permits (Id : Session_Id) return int is
      Count : Natural := 0;
   begin
      for I in 0 .. MAX_PERMITS - 1 loop
         if Permits (I).Magic = PERMIT_MAGIC
            and Permits (I).Session = Id
         then
            Permits (I).Magic := 0;
            Permits (I).Used  := True;
            Count := Count + 1;
         end if;
      end loop;
      if Count = 0 then
         return ERR_NO_PERMIT;
      end if;
      return ERR_OK;
   end Revoke_Session_Permits;

   -- -------------------------------------------------------------------------
   -- Admin_Permit — kernel-only unconditional permit
   -- -------------------------------------------------------------------------

   function Admin_Permit
      (LBA_Start : LBA_Type;
       LBA_End   : LBA_Type) return int
   is
      Pidx : Integer;
   begin
      Pidx := Find_Free_Permit;
      if Pidx = -1 then
         return ERR_FULL;
      end if;

      Permits (Pidx).Magic       := PERMIT_MAGIC;
      Permits (Pidx).Session     := 0;
      Permits (Pidx).LBA_Start   := LBA_Start;
      Permits (Pidx).LBA_End     := LBA_End;
      Permits (Pidx).Flags       := PERMIT_READ or PERMIT_WRITE
                                    or PERMIT_VERIFY or PERMIT_ADMIN;
      Permits (Pidx).Ring        := 0;
      Permits (Pidx).Used        := False;
      Permits (Pidx).Tick_Issued := State.Tick;
      Permits (Pidx).Tick_Expiry := LBA_Type'Last;  -- never expires
      for J in 0 .. TOKEN_SIZE - 1 loop
         Permits (Pidx).Token (J) := 0;
      end loop;
      Permits (Pidx).Checksum := CRC32_Permit (Permits (Pidx));

      State.Total_Permits := State.Total_Permits + 1;
      return ERR_OK;
   end Admin_Permit;

   -- -------------------------------------------------------------------------
   -- Active_Sessions
   -- -------------------------------------------------------------------------

   function Active_Sessions return int is
   begin
      return int (State.Session_Count);
   end Active_Sessions;

end DiskAuth;