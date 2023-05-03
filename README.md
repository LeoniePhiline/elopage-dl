# elopage-dl
*Watch your elopage course on the airplane.*

## Why does this exist?
There are still places in this world where you have no internet connection and thus cannot keep on learning all the good stuff you purchased on elopage.

This little helper makes it comfortable to fetch the course contents (videos, PDFs, and other files) of your purchased elopage online course to disk, so that you can load them onto your phone or tablet and study on the airplane.

## So is this a hacking tool?
No. There is no hacking involved. This tool does not give you access to anything you would not otherwise have access to.

What it does is really quite trivial, and you could do it by hand using your browser devtool's network tab. It would just take you longer - and why do it by hand if you can automate the task?

## Your responsibility
Please be aware that lots of small-scale solo-entrepreneur teachers and coaches make their living by gathering expertise, creating online courses - which takes a considerable amount of effort and time to do! - and offering them for other people to learn from.

**Respect their work!**

This also means that if you purchased a time-based license, you *must delete* the offline-cached course contents when your license expires!

## So how do I use this?

### Get the source, get the toolchain and build your binaries

No prebuilt binaries are provided. To compile the tool, simply get `rustup` and `cargo` by following the instructions at [rustup.rs](https://rustup.rs/).

Then `git clone` this repository (or download as `.zip` file and extract), and run `cargo build --release` in the project folder. Cargo will make sure to download all dependencies from [crates.io](https://crates.io), install and compile them; then it will compile the app for you.

The finished executable binary will be found at `<project folder>/target/release/elopage-dl` on Linux or Mac,
or at `<project folder>/target/release/elopage-dl.exe` on Windows. (Note that Windows might not work out of the box. Try it and [open an Issue](https://github.com/LeoniePhiline/elopage-dl/issues/new) if it does not work.)

To offline-cache your elopage course for your offline airplane journey, you will run the executable in your terminal. However, first you must gather some information and your elopage API authorization token.

### Gather the required information and auth token

#### Course ID

Open your browser, and on a new tab, open its developer tools (commonly you can use the `F12`) key. Switch to the *Network* panel. 

Log into elopage and navigate to the course you want to study while offline.

In the address bar, you will find a URL in the form of `https://elopage.com/payer/s/<SELLER USERNAME>/courses/<COURSE SLUG>?course_session_id=<SOME NUMBERS>`

Copy the numeric course ID which follows `?course_session_id=`.

#### Auth token

Now, back to your browser's developer tools panel:

In the *Network* tab, pick one of the requests going out to `api.elopage.com`.

You can help yourself to find an appropriate request:

1. Toggle the `XHR` request type filter, and / or 
2. Type `api.elopage.com` into the request search box.

After clicking the request in the network requests list, you will see *Response Headers* and *Request Headers*. Under *Request Headers* find `Authorization: ey...` and copy the entire value starting after the `:` (thus, starting with `ey`).

#### Target dir

You will need to provide the directory / folder where your offline cache will live. Get the path to that directory.

In this directory, a structure will be created: `./Elopage/<SELLER USERNAME> (<SELLER FULL NAME)/<COURSE NAME>/`. Each category and each lesson get their own subfolder.

### Start offline-caching

In your terminal, enter the following, while replacing the `<MARKERS>` with the information you gathered above:

```bash
./target/release/elopage-dl --course-id '<COURSE ID>' --token '<AUTH TOKEN>' --output-dir 'path/to/target/directory'
```

You can replace `--course-id` by `-c`, `--token` by `-t` and `--output-dir` by `-o`.

After pressing `Enter`, you should see a bunch of stuff printed into your terminal. Look at it or ignore - the interesting part will happen at your target directory.

You should see the above described folder structure having been created, with course videos and files being downloaded one by one.

## Is it blazingly fast?
No, it's not meant to be. Downloading all files in parallel would be rather trivial, but also a good way to hit the rate limits of either the elopage API or the wistia video source.

## Disclaimer
This little helper has been built to help create a supposedly legal temporary offline cache of your purchased elopage course videos and files, imitating the way a browser would fetch and offline-cache videos while you study the course.

Make sure you hold the copyright of, or another granted license to any material, and tread on safe legal ground according to the country you live in, before you use this.

Respect the work and effort, as well as the copyright of the course sellers. **Never steal or share online course contents! You are badly hurting small-scale businesses if you do!**
