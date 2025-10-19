#rsync -nrv \
#--exclude={target,'Cargo.lock','.*',libraries,bluster} \
#. pi@192.168.5.31://home/pi/hive

rsync  -rv target/armv7-unknown-linux-gnueabihf/release/examples/just_listen \
pi@192.168.5.31:/home/pi/hive/target/release/examples/just_listen

rsync -rv target/armv7-unknown-linux-gnueabihf/release/examples/bluetooth_connect \
pi@192.168.5.80:/home/pi/hive/target/release/examples/bluetooth_connect

