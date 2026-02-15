#ifndef TYPES_H
#define TYPES_H

typedef unsigned char uint8_t;
typedef unsigned short uint16_t;
typedef unsigned int uint32_t;
typedef unsigned long long uint64_t;

typedef signed char int8_t;
typedef signed short int16_t;
typedef signed int int32_t;
typedef signed long long int64_t;

typedef unsigned long long size_t;
typedef signed long long ssize_t;

#define NULL ((void*)0)

#define ALIGN_UP(x, align) (((x) + (align) - 1) & ~((align) - 1))

#endif