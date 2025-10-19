# script that launches maps build directories and launches a Docker container that cross compiles hive for
# Arm architecture that runs on raspberry pi. Dbus is especially tricky: https://github.com/diwic/dbus-rs/issues/184
# the bluetooth feature requires dbus

# Sync my hive repo to remote pi
# rsync -nrv --exclude={target,'Cargo.lock','.*',libraries,bluster} . pi@192.168.5.30://home/pi/hive

#docker run --entrypoint /bin/bash \
# docker run -it --entrypoint /bin/bash \
# docker run \
docker run \
--volume /Users/enochcarter/hive:/home/cross/hive \
--volume /Users/enochcarter/dbus_cross:/home/cross/dbus_cross \
--volume /Users/enochcarter/.cargo/registry:/home/cross/.cargo/registry \
--volume /Users/enochcarter/hive_bt_listen:/home/cross/hive_bt_listen \
 vonamos/rust_berry:latest
