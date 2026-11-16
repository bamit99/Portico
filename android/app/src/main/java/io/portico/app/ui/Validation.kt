package io.portico.app.ui

object Validation {
    fun parseVidPid(input: String): String? {
        val trimmed = input.trim()
        val (v, p) = when {
            '/' in trimmed -> {
                val parts = trimmed.split('/')
                if (parts.size != 2) return null
                parts[0] to parts[1]
            }
            ':' in trimmed -> {
                val parts = trimmed.split(':')
                if (parts.size != 2) return null
                parts[0] to parts[1]
            }
            trimmed.length == 8 && trimmed.all { it.isHexDigit() } -> {
                trimmed.substring(0, 4) to trimmed.substring(4, 8)
            }
            else -> return null
        }
        if (v.length != 4 || p.length != 4 || !v.all { it.isHexDigit() } || !p.all { it.isHexDigit() }) {
            return null
        }
        return "${v.uppercase()}/${p.uppercase()}"
    }

    fun formatAttachedPickerItem(product: String, vidHex: String, pidHex: String): String {
        return "$product(${vidHex.uppercase()}${pidHex.uppercase()})"
    }

    fun normalizeFingerprint(input: String): String? {
        val cleaned = input.filter { it !in charArrayOf(':', '-', ' ', '\t', '\r', '\n') }.lowercase()
        if (cleaned.length != 64 || !cleaned.all { it.isHexDigit() }) return null
        return cleaned
    }

    fun parseReverseClient(addrInput: String, fpInput: String): Pair<String, String>? {
        val addr = addrInput.trim()
        if (addr.any { it == ',' || it == '@' || it.isWhitespace() }) return null
        val colon = addr.lastIndexOf(':')
        if (colon <= 0 || colon >= addr.length - 1) return null
        val host = addr.substring(0, colon)
        val port = addr.substring(colon + 1).toIntOrNull() ?: return null
        if (host.isBlank() || port !in 1..65535) return null
        val fp = normalizeFingerprint(fpInput) ?: return null
        return addr to fp
    }

    fun formatShortFingerprint(fpHex: String): String {
        val clean = fpHex.filter { it.isHexDigit() }.take(16).uppercase()
        if (clean.isEmpty()) return "----"
        return clean.chunked(4).joinToString("-")
    }

    private fun Char.isHexDigit(): Boolean =
        this in '0'..'9' || this in 'a'..'f' || this in 'A'..'F'
}
