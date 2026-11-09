// SPDX-License-Identifier: Apache-2.0
day_bridge::bridge! {
    #[day_bridge::declare]
    extern "day" {
        fn save_native(key:&str,value:&str)->Result<bool,day_bridge::Error>;
        fn load_native(key:&str, done: day_bridge::Done<String>)->Result<(),day_bridge::Error>;
        fn remove_native(key:&str)->Result<bool,day_bridge::Error>;
    }
    #[day_bridge::impl(java, platforms = [android])]
    java!(prelude=r#"
        import android.content.Context;
        import android.content.SharedPreferences;
        import android.security.keystore.KeyGenParameterSpec;
        import android.security.keystore.KeyProperties;
        import android.util.Base64;
        import dev.daybrite.day.bridge.DayBridge;
        import java.security.KeyStore;
        import javax.crypto.Cipher;
        import javax.crypto.KeyGenerator;
        import javax.crypto.SecretKey;
        import javax.crypto.spec.GCMParameterSpec;
        import java.nio.charset.StandardCharsets;
    "#,body=r#"
        private static SharedPreferences prefs() {
            return DayBridge.ctx.getSharedPreferences("day_news_credentials",Context.MODE_PRIVATE);
        }
        private static synchronized SecretKey encryptionKey() throws Exception {
            KeyStore ks=KeyStore.getInstance("AndroidKeyStore"); ks.load(null);
            String alias="day_news_sync_key";
            if (!ks.containsAlias(alias)) {
                KeyGenerator gen=KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES,"AndroidKeyStore");
                gen.init(new KeyGenParameterSpec.Builder(alias,KeyProperties.PURPOSE_ENCRYPT|KeyProperties.PURPOSE_DECRYPT)
                    .setBlockModes(KeyProperties.BLOCK_MODE_GCM).setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE).build());
                gen.generateKey();
            }
            return (SecretKey)ks.getKey(alias,null);
        }
        public static boolean save_native(String key,String value) {
            try {
                Cipher c=Cipher.getInstance("AES/GCM/NoPadding");c.init(Cipher.ENCRYPT_MODE,encryptionKey());
                c.updateAAD(key.getBytes(StandardCharsets.UTF_8));
                String sealed=Base64.encodeToString(c.getIV(),Base64.NO_WRAP)+":"+
                    Base64.encodeToString(c.doFinal(value.getBytes(StandardCharsets.UTF_8)),Base64.NO_WRAP);
                return prefs().edit().putString(key,sealed).commit();
            } catch(Exception e) {return false;}
        }
        private static String loadValue(String key) {
            try {
                String sealed=prefs().getString(key,"");if(sealed.isEmpty()) return "";
                String[] parts=sealed.split(":",2);
                Cipher c=Cipher.getInstance("AES/GCM/NoPadding");
                c.init(Cipher.DECRYPT_MODE,encryptionKey(),new GCMParameterSpec(128,Base64.decode(parts[0],Base64.NO_WRAP)));
                c.updateAAD(key.getBytes(StandardCharsets.UTF_8));
                return new String(c.doFinal(Base64.decode(parts[1],Base64.NO_WRAP)),StandardCharsets.UTF_8);
            } catch(Exception e) {return null;}
        }
        public static void load_native(String key, long done) {String value=loadValue(key);if(value==null) load_native_fail(done,"unavailable");else load_native_complete(done,value);}
        public static boolean remove_native(String key) {
            try {return prefs().edit().remove(key).commit();} catch(Exception e) {return false;}
        }
    "#);
    #[day_bridge::impl(arkts, platforms = [ohos])]
    arkts!(prelude=r#"
        import { asset } from '@kit.AssetStoreKit';
        import { util } from '@kit.ArkTS';
        import { BusinessError } from '@kit.BasicServicesKit';
    "#,body=r#"
        function alias(key: string): asset.AssetMap {
            const q: asset.AssetMap = new Map();
            q.set(asset.Tag.ALIAS,new util.TextEncoder().encodeInto('day-news-sync:'+key));
            return q;
        }
        export function save_native(key: string,value: string): boolean {
            try {
                const q=alias(key);
                q.set(asset.Tag.SECRET,new util.TextEncoder().encodeInto(value));
                q.set(asset.Tag.CONFLICT_RESOLUTION,asset.ConflictResolution.OVERWRITE);
                q.set(asset.Tag.ACCESSIBILITY,asset.Accessibility.DEVICE_FIRST_UNLOCKED);
                asset.addSync(q);return true;
            } catch(e) {return false;}
        }
        function loadValue(key: string): string {
            try {
                const q=alias(key);q.set(asset.Tag.RETURN_TYPE,asset.ReturnType.ALL);
                const rows=asset.querySync(q);
                if(rows.length===0) return '';
                return util.TextDecoder.create('utf-8').decodeToString(rows[0].get(asset.Tag.SECRET) as Uint8Array);
            } catch(e) {if((e as BusinessError).code===24000002) return '';throw new Error('unavailable');}
        }
        export function load_native(key: string, done: number): void {try {load_native_complete(done,loadValue(key));} catch(e) {load_native_fail(done,'unavailable');}}
        export function remove_native(key: string): boolean {
            try {asset.removeSync(alias(key));return true;}
            catch(e) {return (e as BusinessError).code===24000002;}
        }
    "#);
    #[day_bridge::impl(rust, platforms = [other])]
    fn save_native(_key:&str,_value:&str)->Result<bool,day_bridge::Error>{Err(day_bridge::Error::Unsupported)}
    #[day_bridge::impl(rust, platforms = [other])]
    fn load_native(_key:&str,_done:day_bridge::Done<String>)->Result<(),day_bridge::Error>{Err(day_bridge::Error::Unsupported)}
    #[day_bridge::impl(rust, platforms = [other])]
    fn remove_native(_key:&str)->Result<bool,day_bridge::Error>{Err(day_bridge::Error::Unsupported)}
}

pub fn save(key: &str, value: &str) -> Result<bool, day_bridge::Error> {
    save_native(key, value)
}
pub async fn load(key: &str) -> Result<String, day_bridge::Error> {
    load_native_future(key).await
}
pub fn remove(key: &str) -> Result<bool, day_bridge::Error> {
    remove_native(key)
}
