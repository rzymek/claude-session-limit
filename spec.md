Create a very simple Linux Mint desktop app to show current usage of claude code session.
A small borderless window showing:
* progressbar and percentage of 5h session used.
* progressbar till reset in time 
* time till reset in h:m and the reset time in HH:MM (24h clock)

the window is draggable so that it can be moved.
should have low memory footprint.
implementation in Rust if that makes sense.
user in currently logged in into claude cli. the app can use claude cli or require own authentication by opening an auth link in the system browser
