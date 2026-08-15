#!/usr/bin/env python3
"""Checks the comment remover against the text a single line of regex would break on.

    python3 scripts/comments-test.py
"""
import contextlib
import io
import os
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import comments  # noqa: E402


def write(path, text):
    with open(path, "w", encoding="utf-8") as handle:
        handle.write(text)


def clean(text, language, path="<text>"):
    result = comments.clean(text, language, path)
    if result.complaints:
        raise AssertionError(f"{path}: {result.complaints}")
    return result.after


def stay(text, language, path="<text>"):
    result = comments.clean(text, language, path)
    return [text[f.start:f.end] for f in result.finds if f.keep]


class Rust(unittest.TestCase):
    def test_an_address_in_a_string_stays(self):
        source = 'let base = "https://api.modrinth.com/v2";\n'
        self.assertEqual(clean(source, "rust"), source)

    def test_an_address_with_a_comment_behind_it(self):
        source = 'let base = "https://api.modrinth.com/v2"; // the only source\n'
        self.assertEqual(clean(source, "rust"), 'let base = "https://api.modrinth.com/v2";\n')

    def test_a_raw_string_with_hashes(self):
        source = (
            'let sql = r#"SELECT "a//b" FROM t WHERE x = \'/\'"#;\n'
            'let deeper = r##"a "# sits in the middle"##; // away with it\n'
            'let byte = br#"raw // and byte"#;\n'
        )
        self.assertEqual(
            clean(source, "rust"),
            'let sql = r#"SELECT "a//b" FROM t WHERE x = \'/\'"#;\n'
            'let deeper = r##"a "# sits in the middle"##;\n'
            'let byte = br#"raw // and byte"#;\n',
        )

    def test_a_character_literal_and_a_lifetime(self):
        source = (
            "fn split_up<'a>(s: &'a str) -> Vec<&'a str> {\n"
            "    let separator = '/'; // the path separator\n"
            "    let escape = '\\''; // a quotation mark\n"
            "    let raw = b'/';\n"
            "    s.split(separator).collect()\n"
            "}\n"
        )
        self.assertEqual(
            clean(source, "rust"),
            "fn split_up<'a>(s: &'a str) -> Vec<&'a str> {\n"
            "    let separator = '/';\n"
            "    let escape = '\\'';\n"
            "    let raw = b'/';\n"
            "    s.split(separator).collect()\n"
            "}\n",
        )

    def test_a_module_head_at_the_start_of_the_file(self):
        source = (
            "//! The head of the module.\n"
            "//!\n"
            "//! Second line.\n"
            "\n"
            "use std::io;\n"
        )
        self.assertEqual(clean(source, "rust"), "use std::io;\n")

    def test_an_inner_attribute_stays_at_the_top(self):
        source = (
            "//! Head.\n"
            "\n"
            "#![allow(dead_code)]\n"
            "\n"
            "use std::io;\n"
        )
        self.assertEqual(clean(source, "rust"), "#![allow(dead_code)]\n\nuse std::io;\n")

    def test_a_block_over_several_lines_and_nested(self):
        source = (
            "fn f() {\n"
            "    /* first line\n"
            "       second line /* and a block inside it */ still inside */\n"
            "    let x = 1;\n"
            "}\n"
        )
        self.assertEqual(clean(source, "rust"), "fn f() {\n    let x = 1;\n}\n")

    def test_a_block_in_the_middle_of_the_line_becomes_a_space(self):
        source = "let n = a/* count */+ b;\nlet m = c /* x */ - d;\n"
        self.assertEqual(clean(source, "rust"), "let n = a + b;\nlet m = c - d;\n")

    def test_a_comment_above_an_attribute_goes_the_attribute_stays(self):
        source = (
            "// Why this has to be here.\n"
            "#[allow(dead_code)]\n"
            "pub struct A;\n"
        )
        self.assertEqual(clean(source, "rust"), "#[allow(dead_code)]\npub struct A;\n")

    def test_a_string_with_comment_characters(self):
        source = 'let s = "/* no block */ and // no line";\n'
        self.assertEqual(clean(source, "rust"), source)

    def test_four_slashes_are_an_ordinary_comment(self):
        self.assertEqual(clean("//// Divider\nlet a = 1;\n", "rust"), "let a = 1;\n")


class Clap(unittest.TestCase):
    source = (
        "//! The module head that may go.\n"
        "\n"
        "/// The description of the command that clap prints.\n"
        "#[derive(Debug, Subcommand)]\n"
        "pub enum AdminCommand {\n"
        "    /// Creates an administrator.\n"
        "    Create(Create),\n"
        "    // This here is a remark, not help.\n"
        "    Passwd(Passwd),\n"
        "}\n"
        "\n"
        "/// Doc on an ordinary struct.\n"
        "#[derive(Debug, Clone)]\n"
        "pub struct Other {\n"
        "    /// This one goes too.\n"
        "    pub a: u32,\n"
        "}\n"
    )

    def test_the_help_texts_stay(self):
        self.assertEqual(
            clean(self.source, "rust"),
            "/// The description of the command that clap prints.\n"
            "#[derive(Debug, Subcommand)]\n"
            "pub enum AdminCommand {\n"
            "    /// Creates an administrator.\n"
            "    Create(Create),\n"
            "    Passwd(Passwd),\n"
            "}\n"
            "\n"
            "#[derive(Debug, Clone)]\n"
            "pub struct Other {\n"
            "    pub a: u32,\n"
            "}\n",
        )

    def test_the_reason_is_named(self):
        reasons = {f.reason for f in comments.clean(self.source, "rust").finds if f.keep}
        self.assertEqual(reasons, {"clap prints this as --help"})

    def test_the_fields_of_an_args_struct(self):
        source = (
            "#[derive(Debug, Args)]\n"
            "pub struct Create {\n"
            "    #[arg(long)]\n"
            "    pub username: String,\n"
            "    /// An address for the account.\n"
            "    #[arg(long)]\n"
            "    pub email: Option<String>,\n"
            "}\n"
        )
        self.assertEqual(clean(source, "rust"), source)

    def test_a_derive_inside_a_comment_protects_nothing(self):
        source = (
            "//! This is how it looks:\n"
            "//!\n"
            "//! #[derive(Parser)]\n"
            "//! struct Cli {\n"
            "//!     port: u16,\n"
            "//! }\n"
            "\n"
            "/// Goes away.\n"
            "pub fn f() {}\n"
        )
        self.assertEqual(clean(source, "rust"), "pub fn f() {}\n")

    def test_a_string_with_a_brace_does_not_hold_up_the_body(self):
        source = (
            "#[derive(Parser)]\n"
            '#[command(name = "craftpanel", about = "a } in the string")]\n'
            "struct Cli {\n"
            "    /// Help.\n"
            "    a: u8,\n"
            "}\n"
            "\n"
            "/// Goes away.\n"
            "fn f() {}\n"
        )
        self.assertEqual(
            clean(source, "rust"),
            "#[derive(Parser)]\n"
            '#[command(name = "craftpanel", about = "a } in the string")]\n'
            "struct Cli {\n"
            "    /// Help.\n"
            "    a: u8,\n"
            "}\n"
            "\n"
            "fn f() {}\n",
        )


class Doctest(unittest.TestCase):
    source = (
        "//! An example:\n"
        "//!\n"
        "//! ```\n"
        "//! let a = 1;\n"
        "//! ```\n"
        "\n"
        "pub fn a() {}\n"
    )

    def crate(self, folder, with_lib):
        os.makedirs(os.path.join(folder, "src"))
        write(os.path.join(folder, "Cargo.toml"), '[package]\nname = "x"\n')
        path = os.path.join(folder, "src", "lib.rs" if with_lib else "main.rs")
        write(path, self.source)
        return path

    def test_a_library_keeps_the_code_block(self):
        with tempfile.TemporaryDirectory() as folder:
            path = self.crate(folder, True)
            self.assertEqual(clean(self.source, "rust", path), self.source)

    def test_a_binary_crate_has_no_doctests(self):
        with tempfile.TemporaryDirectory() as folder:
            path = self.crate(folder, False)
            self.assertEqual(clean(self.source, "rust", path), "pub fn a() {}\n")


class TypeScript(unittest.TestCase):
    def test_an_address_and_a_comment_behind_it(self):
        source = "const base = 'https://api.modrinth.com' // the source\n"
        self.assertEqual(clean(source, "ts"), "const base = 'https://api.modrinth.com'\n")

    def test_a_pattern_inside_a_string(self):
        source = "export default { content: ['./src/**/*.{js,ts,vue}', '../vendor/**/*.vue'] }\n"
        self.assertEqual(clean(source, "ts"), source)

    def test_a_regular_expression_with_quotation_marks(self):
        source = (
            "const tight = (s: string) => s.replace(/\\s+/g, ' ').replace(/'/g, '\"')\n"
            "const without = base.replace(/\\/+$/, '') // the last slash\n"
            "const parts = text.split(/\\r?\\n/)\n"
        )
        self.assertEqual(
            clean(source, "ts"),
            "const tight = (s: string) => s.replace(/\\s+/g, ' ').replace(/'/g, '\"')\n"
            "const without = base.replace(/\\/+$/, '')\n"
            "const parts = text.split(/\\r?\\n/)\n",
        )

    def test_a_division_is_not_a_regular_expression(self):
        source = "const share = used / total // in parts\nconst rest = (a) / b\n"
        self.assertEqual(clean(source, "ts"), "const share = used / total\nconst rest = (a) / b\n")

    def test_a_template_with_a_substitution(self):
        source = (
            "const away = `https://modrinth.com/${kind}/${slug}` // do not touch\n"
            "const deep = `${a > 1 ? `//${b}` : ''}/end`\n"
        )
        self.assertEqual(
            clean(source, "ts"),
            "const away = `https://modrinth.com/${kind}/${slug}`\n"
            "const deep = `${a > 1 ? `//${b}` : ''}/end`\n",
        )

    def test_instructions_to_a_tool_stay(self):
        source = (
            "// An explanation that may go.\n"
            "// @ts-expect-error the foreign code does not know the field\n"
            "el.style.zoom = '1'\n"
            "// eslint-disable-next-line no-console\n"
            "console.log('x')\n"
        )
        self.assertEqual(
            clean(source, "ts"),
            "// @ts-expect-error the foreign code does not know the field\n"
            "el.style.zoom = '1'\n"
            "// eslint-disable-next-line no-console\n"
            "console.log('x')\n",
        )
        self.assertEqual(len(stay(source, "ts")), 2)

    def test_jsdoc_over_several_lines(self):
        source = (
            "/**\n"
            " * What this function does.\n"
            " * @param a the number\n"
            " */\n"
            "export function f(a: number) {\n"
            "\t/** Why this is here. */\n"
            "\n"
            "\treturn a\n"
            "}\n"
        )
        self.assertEqual(clean(source, "ts"), "export function f(a: number) {\n\treturn a\n}\n")


class Vue(unittest.TestCase):
    source = (
        "<template>\n"
        "\t<!-- Why the anchor is here. -->\n"
        "\t<a href=\"https://modrinth.com/mod/fabric-api\" :title=\"'a // b'\">\n"
        "\t\t{{ name }}\n"
        "\t</a>\n"
        "\t<template #footer>\n"
        "\t\t<!-- this one too -->\n"
        "\t\t<span>Foot</span>\n"
        "\t</template>\n"
        "</template>\n"
        "\n"
        "<script setup lang=\"ts\">\n"
        "// The reason for the detour.\n"
        "const name = 'a // b' // and one more\n"
        "</script>\n"
        "\n"
        "<style scoped lang=\"scss\">\n"
        "// Why this rule is needed.\n"
        ".a {\n"
        "\tbackground: url(https://example.test/x.png); /* the background */\n"
        "}\n"
        "</style>\n"
    )

    def test_three_languages_in_one_file(self):
        self.assertEqual(
            clean(self.source, "vue"),
            "<template>\n"
            "\t<a href=\"https://modrinth.com/mod/fabric-api\" :title=\"'a // b'\">\n"
            "\t\t{{ name }}\n"
            "\t</a>\n"
            "\t<template #footer>\n"
            "\t\t<span>Foot</span>\n"
            "\t</template>\n"
            "</template>\n"
            "\n"
            "<script setup lang=\"ts\">\n"
            "const name = 'a // b'\n"
            "</script>\n"
            "\n"
            "<style scoped lang=\"scss\">\n"
            ".a {\n"
            "\tbackground: url(https://example.test/x.png);\n"
            "}\n"
            "</style>\n",
        )

    def test_a_comment_inside_the_text_of_a_line(self):
        source = "<template>\n\t<p>front <!-- away --> back</p>\n</template>\n"
        self.assertEqual(clean(source, "vue"), "<template>\n\t<p>front back</p>\n</template>\n")


class MailTemplate(unittest.TestCase):
    def test_conditional_comments_stay(self):
        source = (
            "<body>\n"
            "\t<!-- The preview line in the inbox. -->\n"
            "\t<!--[if mso]>\n"
            "\t<table role=\"presentation\"><tr><td>\n"
            "\t<![endif]-->\n"
            "\t<div>{{preheader}}</div>\n"
            "\t<!--[if !mso]><!-->\n"
            "\t<div class=\"modern\">x</div>\n"
            "\t<!--<![endif]-->\n"
            "</body>\n"
        )
        self.assertEqual(
            clean(source, "html"),
            "<body>\n"
            "\t<!--[if mso]>\n"
            "\t<table role=\"presentation\"><tr><td>\n"
            "\t<![endif]-->\n"
            "\t<div>{{preheader}}</div>\n"
            "\t<!--[if !mso]><!-->\n"
            "\t<div class=\"modern\">x</div>\n"
            "\t<!--<![endif]-->\n"
            "</body>\n",
        )

    def test_a_style_block_in_the_template(self):
        source = (
            "<html>\n<head>\n<style>\n"
            "/* Outlook.com moves the line height otherwise. */\n"
            ".ExternalClass {\n\twidth: 100%;\n}\n"
            "</style>\n</head>\n</html>\n"
        )
        self.assertEqual(
            clean(source, "html"),
            "<html>\n<head>\n<style>\n.ExternalClass {\n\twidth: 100%;\n}\n</style>\n</head>\n</html>\n",
        )


class Migrations(unittest.TestCase):
    def test_sql_is_not_touched(self):
        with tempfile.TemporaryDirectory() as folder:
            write(os.path.join(folder, "0001_x.sql"), "-- why\nCREATE TABLE a (b TEXT);\n")
            write(os.path.join(folder, "a.rs"), "// away\nfn f() {}\n")
            to_read, left_alone = comments.files([folder])
            self.assertEqual([os.path.basename(p) for p in to_read], ["a.rs"])
            self.assertEqual([os.path.basename(p) for p in left_alone], ["0001_x.sql"])


class Style(unittest.TestCase):
    def test_css_knows_no_line_comments(self):
        source = ".a {\n\tbackground: url(https://example.test/x.png);\n}\n"
        self.assertEqual(clean(source, "css"), source)

    def test_scss_knows_them(self):
        source = "// Why.\n.a {\n\tcolor: red; // here\n}\n"
        self.assertEqual(clean(source, "scss"), ".a {\n\tcolor: red;\n}\n")


class BlankLines(unittest.TestCase):
    def test_never_two_in_a_row(self):
        source = "let a = 1;\n\n// away\n\nlet b = 2;\n"
        self.assertEqual(clean(source, "rust"), "let a = 1;\n\nlet b = 2;\n")

    def test_none_at_the_start_of_a_block(self):
        source = "fn f() {\n    // away\n\n    let a = 1;\n\n    // away too\n}\n"
        self.assertEqual(clean(source, "rust"), "fn f() {\n    let a = 1;\n}\n")

    def test_none_at_the_start_and_at_the_end_of_the_file(self):
        source = "// away\n\nlet a = 1;\n\n// and away\n"
        self.assertEqual(clean(source, "rust"), "let a = 1;\n")

    def test_the_layout_stays(self):
        source = (
            "fn a() {}\n"
            "\n"
            "/// About b.\n"
            "fn b() {}\n"
            "\n"
            "fn c() {}\n"
        )
        self.assertEqual(clean(source, "rust"), "fn a() {}\n\nfn b() {}\n\nfn c() {}\n")

    def test_blank_lines_without_a_seam_are_left_untouched(self):
        source = "let a = 1;\n\n\n\nlet b = 2; // away\n"
        self.assertEqual(clean(source, "rust"), "let a = 1;\n\n\n\nlet b = 2;\n")


class Nets(unittest.TestCase):
    examples = [
        (Vue.source, "vue"),
        (Clap.source, "rust"),
        ("let s = \"//\"; // away\n", "rust"),
        ("const a = /'/ // away\n", "ts"),
    ]

    def test_running_twice_changes_nothing_more(self):
        for text, language in self.examples:
            once = clean(text, language)
            self.assertEqual(clean(once, language), once, language)

    def test_no_code_and_no_string_is_lost(self):
        for text, language in self.examples:
            result = comments.clean(text, language)
            self.assertEqual(result.complaints, [])
            self.assertEqual(
                comments.skeleton(text, language)[0].replace("//away", ""),
                comments.skeleton(result.after, language)[0].replace("//away", ""),
            )

    def test_an_open_string_is_complained_about(self):
        result = comments.clean("const a = 'open\n", "ts")
        self.assertTrue(result.complaints)


class Guard(unittest.TestCase):
    def tree(self, folder, content):
        os.makedirs(os.path.join(folder, "crates", "x", "src"))
        os.makedirs(os.path.join(folder, "web", "src"))
        write(os.path.join(folder, "crates", "x", "Cargo.toml"), "[package]\n")
        write(os.path.join(folder, "crates", "x", "src", "main.rs"), content)
        write(os.path.join(folder, "web", "src", "a.ts"), "export const a = 1\n")
        return [os.path.join(folder, "crates"), os.path.join(folder, "web", "src")]

    def guard(self, paths):
        with contextlib.redirect_stdout(io.StringIO()) as output:
            state = comments.main(["--check"] + paths)
        return state, output.getvalue()

    def test_a_clean_tree_is_green(self):
        with tempfile.TemporaryDirectory() as folder:
            paths = self.tree(folder, "fn main() {}\n")
            self.assertEqual(self.guard(paths)[0], 0)

    def test_one_comment_makes_it_red(self):
        with tempfile.TemporaryDirectory() as folder:
            paths = self.tree(folder, "// one more again\nfn main() {}\n")
            state, output = self.guard(paths)
            self.assertEqual(state, 1)
            self.assertIn("main.rs:1: // one more again", output)

    def test_the_ones_a_tool_acts_on_stay_green(self):
        with tempfile.TemporaryDirectory() as folder:
            paths = self.tree(
                folder,
                "#[derive(Parser)]\nstruct Cli {\n    /// Help.\n    a: u8,\n}\n\nfn main() {}\n",
            )
            self.assertEqual(self.guard(paths)[0], 0)


if __name__ == "__main__":
    unittest.main(verbosity=2)
