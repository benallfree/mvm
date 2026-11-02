
### Install

macOS and Linux, with Homebrew:

```
brew tap benallfree/mvm
brew install mvm
```

Debian and Ubuntu, with apt:

```
curl -fsSL https://benallfree.github.io/mvm-apt/mvm.gpg | sudo tee /usr/share/keyrings/mvm.gpg > /dev/null
echo "deb [signed-by=/usr/share/keyrings/mvm.gpg] https://benallfree.github.io/mvm-apt stable main" | sudo tee /etc/apt/sources.list.d/mvm.list
sudo apt update && sudo apt install mvm
```

Windows, with winget (a new version can take a day to appear):

```
winget install mvm
```

Or download an archive below and put `mvm` on your `PATH`.

### Upgrade

```
brew upgrade mvm                                      # Homebrew
sudo apt update && sudo apt install --only-upgrade mvm  # apt
winget upgrade mvm                                    # winget
```
