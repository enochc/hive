package com.enochc.hive

import java.util.concurrent.atomic.AtomicLong

/** How much of Hive's own logging to show, see [NativeHive.initLogging]. */
enum class HiveLogLevel { OFF, ERROR, WARN, INFO, DEBUG, TRACE }

/** One entry of a server's peer list. */
data class Peer(val name: String, val address: String)

/**
 * What a [NativeHive] reports. Every method is called on the hive's events
 * thread, one at a time and in the order things happened, so hand off to
 * your own thread (the Android main thread, a coroutine) before touching
 * UI. Calling back into the hive from here is fine.
 */
interface HiveListener {
    /**
     * A property has a new value, or was deleted when [value] is null.
     * [value] is a [String], [Boolean], [Long] or [Double].
     *
     * Only ids travel with values, so [name] is null for a property this
     * hive hasn't been told the name of. A name can come from the
     * `[Properties]` table of the config, from [NativeHive.set] or
     * [NativeHive.registerName], or from a peer after
     * [NativeHive.requestNames], in which case [onPropertyName] follows.
     */
    fun onProperty(id: Long, name: String?, value: Any?) {}

    /**
     * The hive learned what a property is called. [onProperty] carries the
     * name from here on; a property reported earlier without it is the
     * same one.
     */
    fun onPropertyName(id: Long, name: String) {}

    /**
     * The peer list of the server this hive is connected to, after
     * [NativeHive.requestPeers] and again whenever it changes. The first
     * entry is the server itself.
     */
    fun onPeers(peers: List<Peer>) {}

    /** A message another hive sent to this one, see [NativeHive.sendToPeer]. */
    fun onPeerMessage(message: String) {}

    /** A peer attached or went away. [peers] is how many are attached now. */
    fun onConnection(connected: Boolean, peers: Int) {}
}

/**
 * A Hive running inside this process, the Rust implementation loaded from
 * `libhive.so` through JNI (src/jni_api.rs in the hive crate, which this
 * file must stay in step with).
 *
 * [config] is the TOML a Rust hive takes, for a client something like
 *
 *     name = "Phone"
 *     connect = "192.168.1.10:3000"
 *     [Properties]
 *     lightValue = 0
 *
 * The constructor returns straight away and the hive keeps trying to reach
 * its server in the background; [HiveListener.onConnection] says when it
 * has. A hive does not reconnect once its server goes away: [close] it and
 * make a new one. Throws [IllegalArgumentException] if the config is
 * refused.
 *
 * Methods can be called from any thread. They don't wait for the network,
 * though a caller setting properties faster than they can be sent is held
 * back. After [close] they do nothing.
 *
 * With code shrinking on (R8/ProGuard), keep this package: the native side
 * finds the callbacks by name.
 *
 *     -keep class com.enochc.hive.** { *; }
 */
class NativeHive(config: String, listener: HiveListener) : AutoCloseable {
    private val handle = AtomicLong(nativeStart(config, Callbacks(listener)))

    fun set(name: String, value: String) = nativeSetString(handle.get(), 0, name, value)
    fun set(name: String, value: Boolean) = nativeSetBoolean(handle.get(), 0, name, value)
    fun set(name: String, value: Long) = nativeSetLong(handle.get(), 0, name, value)
    fun set(name: String, value: Double) = nativeSetDouble(handle.get(), 0, name, value)

    /** Set a property known only by the id it arrived with. */
    fun set(id: Long, value: String) = nativeSetString(handle.get(), id, null, value)
    fun set(id: Long, value: Boolean) = nativeSetBoolean(handle.get(), id, null, value)
    fun set(id: Long, value: Long) = nativeSetLong(handle.get(), id, null, value)
    fun set(id: Long, value: Double) = nativeSetDouble(handle.get(), id, null, value)

    /** Delete a property here and on every connected hive. */
    fun delete(name: String) = nativeDelete(handle.get(), 0, name)
    fun delete(id: Long) = nativeDelete(handle.get(), id, null)

    /**
     * Tell the hive what a property is called, so its events carry the
     * name and hives exchanging names with it learn it too. Returns the
     * property's id.
     */
    fun registerName(name: String): Long = nativeRegisterName(handle.get(), name)

    /**
     * Ask the hives this one is connected to, now or later, what their
     * properties are called, and tell them what ours are. Each name arrives
     * through [HiveListener.onPropertyName].
     *
     * Only for peers running Hive 0.3.0 or newer: older versions stop on a
     * message they don't know.
     */
    fun requestNames() = nativeRequestNames(handle.get())

    /**
     * Send [message] to the hive called [peer]: the server, or another of
     * its clients. Delivery isn't confirmed. False if this hive has closed.
     */
    fun sendToPeer(peer: String, message: String): Boolean = nativeSendToPeer(handle.get(), peer, message)

    /**
     * Ask for [HiveListener.onPeers]: the current list, then every change.
     * Can be called before the connection is up.
     */
    fun requestPeers() = nativeRequestPeers(handle.get())

    val isConnected: Boolean get() = nativeIsConnected(handle.get())
    val peerCount: Int get() = nativePeerCount(handle.get())

    /**
     * Disconnect and shut down. Events already queued are delivered before
     * this returns, unless it is called from a listener method.
     */
    override fun close() = nativeStop(handle.getAndSet(0))

    /** What the native side calls. Not for use from Kotlin. */
    private class Callbacks(private val listener: HiveListener) {
        fun onProperty(id: Long, name: String?, value: Any?) = listener.onProperty(id, name, value)

        fun onPropertyName(id: Long, name: String) = listener.onPropertyName(id, name)

        fun onPeers(names: Array<String>, addresses: Array<String>) =
            listener.onPeers(names.indices.map { Peer(names[it], addresses[it]) })

        fun onPeerMessage(message: String) = listener.onPeerMessage(message)

        fun onConnection(connected: Boolean, peers: Int) = listener.onConnection(connected, peers)
    }

    companion object {
        init {
            System.loadLibrary("hive")
        }

        /**
         * Turn on the native library's own logging: what Hive itself
         * reports about connections, handshakes and messages, and any
         * panic. On Android it goes to logcat under the tag `hive`,
         * elsewhere to standard output. Without this call Hive logs
         * nothing.
         *
         * It applies to every hive in the process. Only the first call
         * takes effect, so call it once, early. [HiveLogLevel.DEBUG] and
         * [HiveLogLevel.TRACE] write several lines for every property
         * update.
         */
        @JvmStatic
        fun initLogging(level: HiveLogLevel = HiveLogLevel.INFO) = nativeInitLogging(level.ordinal)

        /** The id [name] has on the wire. Ids use all 64 bits, so may be negative. */
        @JvmStatic
        fun propertyId(name: String): Long = nativePropertyId(name)

        @JvmStatic private external fun nativeStart(config: String, callbacks: Any): Long
        @JvmStatic private external fun nativeStop(handle: Long)
        @JvmStatic private external fun nativeSetString(handle: Long, id: Long, name: String?, value: String)
        @JvmStatic private external fun nativeSetBoolean(handle: Long, id: Long, name: String?, value: Boolean)
        @JvmStatic private external fun nativeSetLong(handle: Long, id: Long, name: String?, value: Long)
        @JvmStatic private external fun nativeSetDouble(handle: Long, id: Long, name: String?, value: Double)
        @JvmStatic private external fun nativeDelete(handle: Long, id: Long, name: String?)
        @JvmStatic private external fun nativeRegisterName(handle: Long, name: String): Long
        @JvmStatic private external fun nativeSendToPeer(handle: Long, peer: String, message: String): Boolean
        @JvmStatic private external fun nativeRequestPeers(handle: Long)
        @JvmStatic private external fun nativeRequestNames(handle: Long)
        @JvmStatic private external fun nativeIsConnected(handle: Long): Boolean
        @JvmStatic private external fun nativePeerCount(handle: Long): Int
        @JvmStatic private external fun nativePropertyId(name: String): Long
        @JvmStatic private external fun nativeInitLogging(level: Int)
    }
}
