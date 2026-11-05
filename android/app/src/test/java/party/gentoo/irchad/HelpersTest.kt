package party.gentoo.irchad

import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class HelpersTest {
    @Test
    fun readsAtMostTheLimit() {
        val b = ByteArray(200_000) { it.toByte() }
        assertArrayEquals(b, readAtMost(b.inputStream(), b.size))
        assertNull(readAtMost(b.inputStream(), b.size - 1))
        assertEquals(0, readAtMost(ByteArray(0).inputStream(), 0)!!.size)
    }

    @Test
    fun resamples() {
        val p = Pcm(shortArrayOf(0, 1000, 2000, 3000), 24_000)
        val up = resample(p, 48_000)
        assertEquals(8, up.size)
        assertEquals(500, up[1].toInt())
        assertEquals(3000, up.last().toInt())
        assertEquals(2, resample(p, 12_000).size)
        assertTrue(resample(Pcm(ShortArray(0), 8000), 48_000).isEmpty())
    }

    @Test
    fun imagesByMagic() {
        assertTrue(isImage(byteArrayOf(0xFF.toByte(), 0xD8.toByte(), 0xFF.toByte(), 0)))
        assertFalse(isImage("<svg".toByteArray()))
    }

    @Test
    fun gifCommand() {
        assertEquals("cats", gifCommand("/gif cats"))
        assertEquals("cute cats", gifCommand(" /GIF  cute cats "))
        assertEquals("", gifCommand("/gif"))
        assertNull(gifCommand("/gifs cats"))
        assertNull(gifCommand("a /gif"))
    }

    @Test
    fun levels() {
        assertEquals(SILENT_DB, dbfs(ShortArray(480), 480))
        assertEquals(SILENT_DB, dbfs(ShortArray(0), 0))
        assertEquals(0f, dbfs(ShortArray(480) { -32768 }, 480), 0.01f)
        assertEquals(-6.02f, dbfs(ShortArray(480) { 16384 }, 480), 0.01f)
    }
}
