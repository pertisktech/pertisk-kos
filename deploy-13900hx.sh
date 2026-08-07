
./image/fetch-kernel.sh
PERTISK_EMBED_BOOT=1 ./image/build-initramfs.sh
./scripts/deploy-mgmt-lab.sh --mgmt almalinux@10.1.1.150 \
 --cp-gb 50 --worker-gb 75  --version 0.1.48

scp out/pertisk-cloud-amd64{,-50g,-75g}.qcow2 almalinux@10.1.1.150:/tmp/
ssh almalinux@10.1.1.150 'sudo mv /tmp/pertisk-cloud-amd64*.qcow2 /var/lib/pertisk-mgmt/images/ && sudo chown -R pertisk-mgmt:pertisk-mgmt /var/lib/pertisk-mgmt/images'
#scp out/rpm/pertisk-mgmt-0.1.22-1.x86_64.rpm almalinux@10.1.1.150:/tmp
docker system prune -a -f