package ing.zork.android

import android.graphics.BitmapFactory
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import com.google.zxing.BinaryBitmap
import com.google.zxing.RGBLuminanceSource
import com.google.zxing.common.HybridBinarizer
import com.google.zxing.qrcode.QRCodeReader
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File

@RunWith(AndroidJUnit4::class)
class PhoneEnrollmentTest {
    private val context=InstrumentationRegistry.getInstrumentation().targetContext
    private val root=context.noBackupFilesDir.resolve("phone-enrollment-test").absolutePath
    private fun call(op:String,vararg values:Pair<String,Any?>):JSONObject {
        val body=JSONObject().put("op",op);values.forEach { (k,v)->body.put(k,v ?: JSONObject.NULL) }
        android.util.Log.i("ZorkEnrollmentTest", "begin $op")
        val response=JSONObject(NativeBridge.call(root,body.toString()))
        android.util.Log.i("ZorkEnrollmentTest", "end $op")
        assertTrue(response.toString(),response.getBoolean("ok"));return response.getJSONObject("data")
    }
    @Test fun decodeDesktopQrAndRequestApproval() {
        NativeBridge.initialize(context)
        // The image is a real native desktop screenshot, including its UI chrome.
        val bitmap=BitmapFactory.decodeFile("/data/local/tmp/zork-phone-qr.png")
        val pixels=IntArray(bitmap.width*bitmap.height);bitmap.getPixels(pixels,0,bitmap.width,0,0,bitmap.width,bitmap.height)
        val ticket=QRCodeReader().decode(BinaryBitmap(HybridBinarizer(RGBLuminanceSource(bitmap.width,bitmap.height,pixels)))).text
        bitmap.recycle();assertTrue(ticket.startsWith("zc1_"))
        val raw=android.util.Base64.decode(ticket.substring(4),android.util.Base64.URL_SAFE or android.util.Base64.NO_PADDING or android.util.Base64.NO_WRAP)
        android.util.Log.i("ZorkEnrollmentTest","ticket bytes=${raw.size}; hint=${if(raw.size>48)raw[48] else 0}")
        if (raw.size>=55 && raw[48].toInt() and 2 != 0) android.util.Log.i("ZorkEnrollmentTest","address=${java.net.InetAddress.getByAddress(raw.copyOfRange(49,53)).hostAddress}:${((raw[53].toInt() and 255) shl 8)+(raw[54].toInt() and 255)}")
        if (!call("resume").isNull("invitation")) call("cancel_invitation")
        call("begin_invitation","ticket" to ticket,"name" to "Android 扫码验证")
        val result=call("poll_invitation")
        assertEquals("awaiting_approval",result.getJSONObject("invitation").getString("status"))
        assertEquals(0,result.getJSONArray("nodes").length())
        File(context.filesDir,"phone-enrollment-report.json").writeText(JSONObject().put("identity",result.getString("identity")).toString())
        call("pause")
    }
    @Test fun finishAfterDesktopApprovalAndReconnect() {
        NativeBridge.initialize(context)
        val original=JSONObject(File(context.filesDir,"phone-enrollment-report.json").readText()).getString("identity")
        assertEquals(original,call("resume").getString("identity"))
        val result=call("poll_invitation")
        assertTrue(result.isNull("invitation"));val peer=result.getString("joined_peer")
        val read=call("read","peer" to peer,"path" to "/v1/node/agents")
        assertFalse(read.optBoolean("cached"))
        call("pause");call("resume")
        assertFalse(call("read","peer" to peer,"path" to "/v1/node/agents").optBoolean("cached"))
        call("remove_peer","peer" to peer)
        call("pause")
    }
}
