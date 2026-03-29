\ pipeline.fs - GPU Pipeline State
\ Defines registers for shaders and viewport

constant SPI_SHADER_PGM_LO_VS  0x2E40  \ Vertex Shader address
constant SPI_SHADER_PGM_LO_PS  0x2E00  \ Pixel Shader address

\ Set the address of the Vertex Shader
: set-vs-entry ( phys-addr -- )
    SPI_SHADER_PGM_LO_VS mmio-write ;

\ Bind a graphics pipeline state
: bind-pipeline ( pipeline-ptr -- )
    \ Iterate through state registers and apply
    @ execute ;