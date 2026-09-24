package ing.zork.android

import android.text.SpannableString
import android.text.Spanned
import android.text.style.ForegroundColorSpan
import java.util.Locale

/** Bounded lexical coloring for message snippets; no regex backtracking, UI
 * work per draw, language downloads, or network execution. Unknown code is plain. */
internal object MessageCodeColors {
    private const val Keyword = 0xFFA71D5D.toInt()
    private const val StringColor = 0xFF183691.toInt()
    private const val Comment = 0xFF969896.toInt()
    private const val Number = 0xFF0086B3.toInt()
    private const val Function = 0xFF63A35C.toInt()
    private data class Token(val start: Int, val end: Int, val color: Int)
    /** Cached tokens keep the light colors as kinds; dark surfaces get lighter inks. */
    private fun onDark(color: Int): Int = when (color) {
        Keyword -> 0xFFF28FAD.toInt()
        StringColor -> 0xFF9CC9F5.toInt()
        Comment -> 0xFF8A8F95.toInt()
        Number -> 0xFF7FD4E6.toInt()
        Function -> 0xFFA6D48A.toInt()
        else -> color
    }
    private val cache = MarkdownBoundedCache<Pair<String, String>, List<Token>>(96, 4 * 1024 * 1024)
    var tokenizations = 0L; private set
    val cachedEntries get() = cache.size
    val estimatedBytes get() = cache.estimatedBytes
    private val common = "if else for while do return break continue throw try catch finally new true false null class interface public private protected static import package extends implements this super void const var let function async await switch case default in as is".split(' ').toSet()
    private val words = mapOf(
        "rust" to "fn pub mod use crate self Self impl trait struct enum match mut ref move unsafe extern where type dyn async await loop while for in if else return break continue let const static true false Some None Ok Err",
        "kotlin" to "fun val var when object companion data sealed override open suspend inline reified out by init constructor get set null true false import package class interface if else return is as in",
        "python" to "def class import from as if elif else while for in try except finally with lambda return yield raise pass break continue and or not is None True False async await global nonlocal del assert",
        "shell" to "if then else elif fi for do done while case esac function in export local readonly return exit echo printf cd set source test",
        "sql" to "select from where join left right inner outer on as group order by having limit insert into values update set delete create table alter drop null not and or distinct union case when then else end",
        "json" to "true false null",
        "yaml" to "true false null yes no"
    ).mapValues { it.value.split(' ').toSet() }
    private val known = setOf("rust", "kotlin", "python", "shell", "sql", "json", "yaml", "java", "javascript", "typescript", "c", "cpp", "csharp", "go", "swift", "css")
    fun language(info: String?): String = when (val name = info.orEmpty().trim().substringBefore(' ').lowercase(Locale.ROOT)) {
        "rs" -> "rust"; "kt", "kts" -> "kotlin"; "py" -> "python"
        "sh", "bash", "zsh" -> "shell"; "js", "jsx" -> "javascript"; "ts", "tsx" -> "typescript"
        "yml" -> "yaml"; "c++" -> "cpp"; "cs" -> "csharp"; else -> name
    }
    fun highlight(info: String?, source: String): CharSequence {
        val language = language(info)
        if (language !in known || source.length > 65536) return source
        val key = language to source
        val tokens = cache.getOrPut(key, source.length * 40 + 128) {
            if (source.lineSequence().any { it.length > 4096 }) emptyList() else { tokenizations++; color(language, source) }
        }
        if (tokens.isEmpty()) return source
        // Android identifies spans by object identity. Reusing the same span in
        // two identical blocks would move its first occurrence to the second.
        return SpannableString(source).apply {
            val dark = ZorkColors.dark
            tokens.forEach { setSpan(ForegroundColorSpan(if (dark) onDark(it.color) else it.color), it.start, it.end, Spanned.SPAN_EXCLUSIVE_EXCLUSIVE) }
        }
    }
    private fun color(language: String, source: String): List<Token> {
        val result = mutableListOf<Token>()
        val keywords = words[language] ?: common
        fun mark(start: Int, end: Int, color: Int) {
            if (end > start) result.add(Token(start, end, color))
        }
        val slashComments = language !in setOf("python", "shell", "json", "yaml", "sql")
        val hashComments = language in setOf("python", "shell", "yaml")
        var i = 0
        while (i < source.length) {
            val start = i
            val ch = source[i]
            val hashComment = ch == '#' && hashComments
            val lineComment = hashComment || slashComments && source.startsWith("//", i) || language == "sql" && source.startsWith("--", i)
            if (lineComment) {
                i = source.indexOf('\n', i).let { if (it < 0) source.length else it }
                mark(start, i, Comment)
            } else if (slashComments && source.startsWith("/*", i)) {
                i += 2; var depth = 1
                while (i < source.length && depth > 0) {
                    if (source.startsWith("*/", i)) { depth--; i += 2 }
                    else if (language == "rust" && source.startsWith("/*", i)) { depth++; i += 2 }
                    else i++
                }
                mark(start, i, Comment)
            } else if (ch == '"' || ch == '\'' || ch == '`') {
                // A Rust lifetime is an identifier, not an unterminated string.
                if (language == "rust" && ch == '\'' && source.getOrNull(i + 1)?.isLetter() == true && source.getOrNull(i + 2) != '\'') { i++; continue }
                val triple = ch != '`' && source.startsWith("$ch$ch$ch", i)
                val delimiter = if (triple) "$ch$ch$ch" else "$ch"
                i += delimiter.length
                while (i < source.length) {
                    if (source[i] == '\\') { i = (i + 2).coerceAtMost(source.length) }
                    else if (source.startsWith(delimiter, i)) { i += delimiter.length; break }
                    else if (!triple && ch != '`' && source[i] == '\n') break
                    else i++
                }
                mark(start, i, StringColor)
            } else if (ch in '0'..'9' && (i == 0 || !source[i - 1].isLetterOrDigit())) {
                i++
                while (i < source.length && (source[i].isLetterOrDigit() || source[i] in "._")) i++
                mark(start, i, Number)
            } else if (ch.isLetter() || ch == '_') {
                i++
                while (i < source.length && (source[i].isLetterOrDigit() || source[i] == '_')) i++
                val word = source.substring(start, i)
                if ((if (language == "sql") word.lowercase(Locale.ROOT) else word) in keywords) mark(start, i, Keyword)
                else {
                    var next = i
                    while (next < source.length && source[next].isWhitespace()) next++
                    if (source.getOrNull(next) == '(' || language == "rust" && source.getOrNull(next) == '!') mark(start, i, Function)
                }
            } else i++
        }
        return result
    }
}
