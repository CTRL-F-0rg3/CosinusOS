const std = @import("std");

const TokenType = enum {
    identifier,
    number,
    plus,
    minus,
    eof,
};

const Token = struct {
    kind: TokenType,
    lexeme: []const u8,
};

const Lexer = struct {
    input: []const u8,
    pos: usize,

    fn peek(self: *Lexer) u8 {
        if (self.pos >= self.input.len) return 0;
        return self.input[self.pos];
    }

    fn advance(self: *Lexer) void {
        self.pos += 1;
    }

    fn nextToken(self: *Lexer) Token {
        const c = self.peek();

        if (c == 0) {
            return Token{ .kind = .eof, .lexeme = "" };
        }

        if (std.ascii.isDigit(c)) {
            const start = self.pos;
            while (std.ascii.isDigit(self.peek())) {
                self.advance();
            }
            return Token{
                .kind = .number,
                .lexeme = self.input[start..self.pos],
            };
        }

        if (c == '+') {
            self.advance();
            return Token{ .kind = .plus, .lexeme = "+" };
        }

        self.advance();
        return Token{ .kind = .identifier, .lexeme = "?" };
    }
};
