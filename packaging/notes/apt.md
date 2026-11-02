
Debian and Ubuntu (amd64 and arm64), with apt:

```
curl -fsSL https://benallfree.github.io/mvm-apt/mvm.gpg | sudo tee /usr/share/keyrings/mvm.gpg > /dev/null
echo "deb [signed-by=/usr/share/keyrings/mvm.gpg] https://benallfree.github.io/mvm-apt stable main" | sudo tee /etc/apt/sources.list.d/mvm.list
sudo apt update && sudo apt install mvm
```
