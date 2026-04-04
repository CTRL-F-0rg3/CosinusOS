-- CosinusOS CryptoFS Layer
-- cryptofs.adb — implementation

package body CryptoFS
   with SPARK_Mode => On
is

   -- -------------------------------------------------------------------------
   -- CRC32 for key slot integrity
   -- -------------------------------------------------------------------------

   type CRC_Tab_T is array (0 .. 255) of Unsigned_32;
   CRC_Tab : constant CRC_Tab_T := (
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
      others => 0
   );

   function CRC32_Bytes_CF (Data : System.Address; Len : Natural) return Unsigned_32 is
      type BA is array (Natural range <>) of Unsigned_8;
      B   : BA (0 .. Len - 1) with Address => Data, Import => True;
      CRC : Unsigned_32 := 16#FFFFFFFF#;
      Idx : Unsigned_32;
   begin
      for I in 0 .. Len - 1 loop
         Idx := (CRC xor Unsigned_32 (B (I))) and 16#FF#;
         CRC := Shift_Right (CRC, 8) xor CRC_Tab (Integer (Idx));
      end loop;
      return CRC xor 16#FFFFFFFF#;
   end CRC32_Bytes_CF;

   function CRC32_Key_Slot (K : Crypto_Key_Slot) return Unsigned_32 is
      type BA is array (Natural range <>) of Unsigned_8;
      KB : BA (0 .. Crypto_Key_Slot'Size / 8 - 1)
         with Address => K'Address, Import => True;
      CRC : Unsigned_32 := 16#FFFFFFFF#;
      Idx : Unsigned_32;
      CS_Off : constant Natural := 80;
   begin
      for I in 0 .. Crypto_Key_Slot'Size / 8 - 1 loop
         if I < CS_Off or I > CS_Off + 3 then
            Idx := (CRC xor Unsigned_32 (KB (I))) and 16#FF#;
            CRC := Shift_Right (CRC, 8) xor CRC_Tab (Integer (Idx));
         end if;
      end loop;
      return CRC xor 16#FFFFFFFF#;
   end CRC32_Key_Slot;

   -- -------------------------------------------------------------------------
   -- SipHash-2-4 (simplified, 64-bit output)
   -- Reference: Aumasson & Bernstein 2012
   -- -------------------------------------------------------------------------

   function SipHash24
      (Key   : Key_Bytes;
       Data  : System.Address;
       Len   : Natural;
       LBA   : LBA_Type) return Unsigned_64
   is
      type BA is array (Natural range <>) of Unsigned_8;
      B : BA (0 .. Len - 1) with Address => Data, Import => True;

      -- Load key halves
      K0 : Unsigned_64 := 0;
      K1 : Unsigned_64 := 0;

      V0, V1, V2, V3 : Unsigned_64;
      M   : Unsigned_64;
      Acc : Unsigned_64 := 0;

      function Rotl64 (X : Unsigned_64; N : Natural) return Unsigned_64 is
      begin
         return Shift_Left (X, N) or Shift_Right (X, 64 - N);
      end Rotl64;

      procedure SipRound is
      begin
         V0 := V0 + V1;
         V1 := Rotl64 (V1, 13);
         V1 := V1 xor V0;
         V0 := Rotl64 (V0, 32);
         V2 := V2 + V3;
         V3 := Rotl64 (V3, 16);
         V3 := V3 xor V2;
         V0 := V0 + V3;
         V3 := Rotl64 (V3, 21);
         V3 := V3 xor V0;
         V2 := V2 + V1;
         V1 := Rotl64 (V1, 17);
         V1 := V1 xor V2;
         V2 := Rotl64 (V2, 32);
      end SipRound;

   begin
      -- Build K0, K1 from first 16 bytes of key
      for I in 0 .. 7 loop
         K0 := K0 or Shift_Left (Unsigned_64 (Key (I)), I * 8);
         K1 := K1 or Shift_Left (Unsigned_64 (Key (I + 8)), I * 8);
      end loop;

      -- Mix LBA into K1 for per-sector nonce
      K1 := K1 xor LBA;

      V0 := K0 xor 16#736F6D6570736575#;
      V1 := K1 xor 16#646F72616E646F6D#;
      V2 := K0 xor 16#6C7967656E657261#;
      V3 := K1 xor 16#7465646279746573#;

      -- Process 8-byte blocks
      declare
         Full_Blocks : constant Natural := Len / 8;
      begin
         for Block in 0 .. Full_Blocks - 1 loop
            M := 0;
            for J in 0 .. 7 loop
               M := M or Shift_Left (Unsigned_64 (B (Block * 8 + J)), J * 8);
            end loop;
            V3 := V3 xor M;
            SipRound; SipRound;  -- c=2 compression rounds
            V0 := V0 xor M;
         end loop;

         -- Handle remaining bytes
         declare
            Remaining : constant Natural := Len mod 8;
            Last_Off  : constant Natural := Full_Blocks * 8;
         begin
            Acc := Shift_Left (Unsigned_64 (Len mod 256), 56);
            for J in 0 .. Remaining - 1 loop
               Acc := Acc or Shift_Left (Unsigned_64 (B (Last_Off + J)), J * 8);
            end loop;
         end;
      end;

      V3 := V3 xor Acc;
      SipRound; SipRound;
      V0 := V0 xor Acc;

      -- Finalization
      V2 := V2 xor 16#FF#;
      SipRound; SipRound; SipRound; SipRound;  -- d=4 finalization rounds
      return V0 xor V1 xor V2 xor V3;
   end SipHash24;

   -- -------------------------------------------------------------------------
   -- XOR stream cipher — simple LFSR-based keystream
   -- NOT cryptographically secure — used for dev/test and fast integrity
   -- Real deployment should replace with ChaCha20
   -- -------------------------------------------------------------------------

   procedure XOR_Stream
      (Key    : Key_Bytes;
       Nonce  : Nonce_T;
       LBA    : LBA_Type;
       Buffer : System.Address;
       Len    : Natural)
   is
      type BA is array (Natural range <>) of Unsigned_8;
      B : BA (0 .. Len - 1) with Address => Buffer, Import => True;

      -- Build 64-bit state from key + nonce + LBA
      State0 : Unsigned_64 := 0;
      State1 : Unsigned_64 := 0;
      KS     : Unsigned_8;
   begin
      for I in 0 .. 7 loop
         State0 := State0 or Shift_Left (Unsigned_64 (Key (I)), I * 8);
         State1 := State1 or Shift_Left (Unsigned_64 (Key (I + 8)), I * 8);
      end loop;

      -- Mix in nonce
      for I in 0 .. 7 loop
         State0 := State0 xor Shift_Left (Unsigned_64 (Nonce (I mod NONCE_SIZE)), I * 7);
      end loop;

      -- Mix in LBA for sector-unique stream
      State0 := State0 xor LBA;
      State1 := State1 xor Shift_Left (LBA, 3);

      for I in 0 .. Len - 1 loop
         -- xorshift64 step
         State0 := State0 xor Shift_Left  (State0, 13);
         State0 := State0 xor Shift_Right (State0, 7);
         State0 := State0 xor Shift_Left  (State0, 17);
         KS := Unsigned_8 (State0 and 16#FF#) xor
               Unsigned_8 (Shift_Right (State1, (I mod 8) * 8) and 16#FF#);
         B (I) := B (I) xor KS;
      end loop;
   end XOR_Stream;

   -- -------------------------------------------------------------------------
   -- Init
   -- -------------------------------------------------------------------------

   procedure Init is
   begin
      State.Initialized    := False;
      State.Active_Keys    := 0;
      State.Encrypt_Count  := 0;
      State.Decrypt_Count  := 0;
      State.Tag_Fail_Count := 0;
      State.Default_Key    := -1;

      for I in 0 .. MAX_KEY_SLOTS - 1 loop
         Key_Slots (I).Magic           := 0;
         Key_Slots (I).Slot_Id         := 0;
         Key_Slots (I).Cipher          := CIPHER_NONE;
         Key_Slots (I).Flags           := 0;
         Key_Slots (I).LBA_Bound_Start := 0;
         Key_Slots (I).LBA_Bound_End   := 0;
         Key_Slots (I).Use_Count       := 0;
         Key_Slots (I).Checksum        := 0;
         for J in 0 .. KEY_SIZE - 1 loop
            Key_Slots (I).Key (J) := 0;
         end loop;
         for J in 0 .. NONCE_SIZE - 1 loop
            Key_Slots (I).Nonce (J) := 0;
         end loop;
      end loop;

      State.Initialized := True;
   end Init;

   -- -------------------------------------------------------------------------
   -- Find_Key_For_LBA
   -- -------------------------------------------------------------------------

   function Find_Key_For_LBA (LBA : LBA_Type) return Integer is
   begin
      -- First pass: region-bound keys
      for I in 0 .. MAX_KEY_SLOTS - 1 loop
         if Key_Slots (I).Magic = CRYPTO_MAGIC
            and (Key_Slots (I).Flags and 16#0001#) /= 0  -- active
            and (Key_Slots (I).Flags and 16#0004#) /= 0  -- region_bound
         then
            if LBA >= Key_Slots (I).LBA_Bound_Start
               and LBA < Key_Slots (I).LBA_Bound_End
            then
               return I;
            end if;
         end if;
      end loop;

      -- Second pass: default (non-bound) key
      if State.Default_Key >= 0 then
         return State.Default_Key;
      end if;

      return -1;
   end Find_Key_For_LBA;

   -- -------------------------------------------------------------------------
   -- Load_Key
   -- -------------------------------------------------------------------------

   function Load_Key
      (Slot_Id   : Unsigned_8;
       Cipher    : Unsigned_8;
       Key_Data  : System.Address;
       Key_Len   : unsigned;
       Nonce     : System.Address;
       Nonce_Len : unsigned) return int
   is
      pragma Unreferenced (Key_Len, Nonce_Len);
      Idx : constant Integer := Integer (Slot_Id);
      type KB is array (0 .. KEY_SIZE  - 1) of Unsigned_8;
      type NB is array (0 .. NONCE_SIZE - 1) of Unsigned_8;
      KD : KB with Address => Key_Data, Import => True;
      ND : NB with Address => Nonce,    Import => True;
   begin
      if Idx < 0 or Idx >= MAX_KEY_SLOTS then
         return ERR_INVALID;
      end if;

      Key_Slots (Idx).Magic   := CRYPTO_MAGIC;
      Key_Slots (Idx).Slot_Id := Slot_Id;
      Key_Slots (Idx).Cipher  := Cipher;
      Key_Slots (Idx).Flags   := 16#0001#;  -- active, not bound
      for J in 0 .. KEY_SIZE  - 1 loop Key_Slots (Idx).Key (J)   := KD (J); end loop;
      for J in 0 .. NONCE_SIZE - 1 loop Key_Slots (Idx).Nonce (J) := ND (J); end loop;
      Key_Slots (Idx).LBA_Bound_Start := 0;
      Key_Slots (Idx).LBA_Bound_End   := 0;
      Key_Slots (Idx).Use_Count       := 0;
      Key_Slots (Idx).Checksum := CRC32_Key_Slot (Key_Slots (Idx));

      State.Active_Keys := State.Active_Keys + 1;
      if State.Default_Key = -1 then
         State.Default_Key := Idx;
      end if;

      return ERR_OK;
   end Load_Key;

   -- -------------------------------------------------------------------------
   -- Bind_Key_To_Region
   -- -------------------------------------------------------------------------

   function Bind_Key_To_Region
      (Slot_Id   : Unsigned_8;
       LBA_Start : LBA_Type;
       LBA_End   : LBA_Type) return int
   is
      Idx : constant Integer := Integer (Slot_Id);
   begin
      if Idx < 0 or Idx >= MAX_KEY_SLOTS then
         return ERR_INVALID;
      end if;
      if Key_Slots (Idx).Magic /= CRYPTO_MAGIC then
         return ERR_NO_KEY;
      end if;
      Key_Slots (Idx).LBA_Bound_Start := LBA_Start;
      Key_Slots (Idx).LBA_Bound_End   := LBA_End;
      Key_Slots (Idx).Flags := Key_Slots (Idx).Flags or 16#0004#;  -- region_bound
      Key_Slots (Idx).Checksum := CRC32_Key_Slot (Key_Slots (Idx));
      return ERR_OK;
   end Bind_Key_To_Region;

   -- -------------------------------------------------------------------------
   -- Tag_Sector — compute authentication tag
   -- -------------------------------------------------------------------------

   function Tag_Sector
      (LBA    : LBA_Type;
       Buffer : System.Address;
       Tag    : System.Address) return int
   is
      KIdx : Integer;
      H    : Unsigned_64;
      type TB is array (0 .. TAG_SIZE - 1) of Unsigned_8;
      T : TB with Address => Tag, Import => True;
   begin
      KIdx := Find_Key_For_LBA (LBA);
      if KIdx = -1 then
         return ERR_NO_KEY;
      end if;

      H := SipHash24 (Key_Slots (KIdx).Key, Buffer, SECTOR_SIZE, LBA);

      -- Store 8-byte hash into first 8 bytes of tag, zero rest
      for I in 0 .. 7 loop
         T (I) := Unsigned_8 (Shift_Right (H, I * 8) and 16#FF#);
      end loop;
      for I in 8 .. TAG_SIZE - 1 loop
         T (I) := 0;
      end loop;

      Key_Slots (KIdx).Use_Count := Key_Slots (KIdx).Use_Count + 1;
      return ERR_OK;
   end Tag_Sector;

   -- -------------------------------------------------------------------------
   -- Verify_Tag
   -- -------------------------------------------------------------------------

   function Verify_Tag
      (LBA    : LBA_Type;
       Buffer : System.Address;
       Tag    : System.Address) return int
   is
      KIdx     : Integer;
      H        : Unsigned_64;
      Expected : Unsigned_64 := 0;
      type TB is array (0 .. TAG_SIZE - 1) of Unsigned_8;
      T : TB with Address => Tag, Import => True;
   begin
      KIdx := Find_Key_For_LBA (LBA);
      if KIdx = -1 then
         return ERR_NO_KEY;
      end if;

      H := SipHash24 (Key_Slots (KIdx).Key, Buffer, SECTOR_SIZE, LBA);

      for I in 0 .. 7 loop
         Expected := Expected or Shift_Left (Unsigned_64 (T (I)), I * 8);
      end loop;

      if H /= Expected then
         State.Tag_Fail_Count := State.Tag_Fail_Count + 1;
         return ERR_BAD_TAG;
      end if;

      return ERR_OK;
   end Verify_Tag;

   -- -------------------------------------------------------------------------
   -- Encrypt_Sector
   -- -------------------------------------------------------------------------

   function Encrypt_Sector
      (LBA    : LBA_Type;
       Buffer : System.Address;
       Tag    : System.Address) return int
   is
      KIdx : Integer;
      Res  : int;
   begin
      KIdx := Find_Key_For_LBA (LBA);
      if KIdx = -1 then
         return ERR_NO_KEY;
      end if;

      -- Compute tag BEFORE encryption (authenticate plaintext)
      Res := Tag_Sector (LBA, Buffer, Tag);
      if Res /= ERR_OK then
         return Res;
      end if;

      -- Encrypt based on cipher type
      case Key_Slots (KIdx).Cipher is
         when CIPHER_NONE =>
            null;  -- passthrough — no encryption
         when CIPHER_XOR | CIPHER_SIPHASH =>
            XOR_Stream (Key_Slots (KIdx).Key,
                        Key_Slots (KIdx).Nonce,
                        LBA, Buffer, SECTOR_SIZE);
         when others =>
            return ERR_CIPHER;
      end case;

      State.Encrypt_Count := State.Encrypt_Count + 1;
      return ERR_OK;
   end Encrypt_Sector;

   -- -------------------------------------------------------------------------
   -- Decrypt_Sector
   -- -------------------------------------------------------------------------

   function Decrypt_Sector
      (LBA    : LBA_Type;
       Buffer : System.Address;
       Tag    : System.Address) return int
   is
      KIdx : Integer;
      Res  : int;
   begin
      KIdx := Find_Key_For_LBA (LBA);
      if KIdx = -1 then
         return ERR_NO_KEY;
      end if;

      -- Decrypt first
      case Key_Slots (KIdx).Cipher is
         when CIPHER_NONE =>
            null;
         when CIPHER_XOR | CIPHER_SIPHASH =>
            -- XOR stream is symmetric
            XOR_Stream (Key_Slots (KIdx).Key,
                        Key_Slots (KIdx).Nonce,
                        LBA, Buffer, SECTOR_SIZE);
         when others =>
            return ERR_CIPHER;
      end case;

      -- Then verify tag against decrypted plaintext
      Res := Verify_Tag (LBA, Buffer, Tag);
      if Res /= ERR_OK then
         State.Tag_Fail_Count := State.Tag_Fail_Count + 1;
         return ERR_BAD_TAG;
      end if;

      State.Decrypt_Count := State.Decrypt_Count + 1;
      return ERR_OK;
   end Decrypt_Sector;

   -- -------------------------------------------------------------------------
   -- Get_Tag_Fail_Count
   -- -------------------------------------------------------------------------

   function Get_Tag_Fail_Count return Unsigned_32 is
   begin
      return State.Tag_Fail_Count;
   end Get_Tag_Fail_Count;

end CryptoFS;