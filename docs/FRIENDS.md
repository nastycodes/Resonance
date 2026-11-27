# Getting Started

This guide is for people invited to use a Resonance group. It covers installation, account setup, messaging, and important privacy details.

## Overview

Resonance is a private chat app for groups managed by a server administrator. To get started, you need an installer and a one-time invitation code from that administrator.

## Before you begin

- Get the Windows installer from your group administrator.
- Ask the administrator for your personal invitation code.
- Choose a strong password of at least 10 characters. Your password unlocks your account identity.

## Install and create an account

1. Download the installer shared by your administrator.
2. Install Resonance on Windows, then open it.
3. Select **Create an account** and enter your username, display name, invitation code, and password.
4. Share your username with people in your group so they can find you.
5. On future launches, sign in with the same username and password.

Each invitation code can be used to create one account only. Do not post your code in a public channel.

## Manage your account

Open **Settings → Account** to change your display name, choose a profile picture, or update your password. Profile pictures are shared with your friends and must be PNG, JPEG, or WebP files no larger than 1 MB.

Changing the password requires your current password and updates both account sign-in and encryption of your identity key. Other active sessions are signed out; use the new password on your next sign-in.

## Find friends and start a conversation

1. Find a person by their username and send a friend request.
2. They must accept your request under **Friends → Requests** before you can chat.
3. Incoming and outgoing requests refresh while you are signed in. You can cancel a request you sent.

Your accepted friends and presence refresh automatically while you are signed in. An account appears online only while it has sent a heartbeat within the last 20 seconds.

## Verify a contact's identity

Open **Identity** on a friend card to view that contact's verification status and manage verification. If the contact is not verified, send a live verification request; the other person must be signed in and accept it while online. Requests are sent directly and are not queued. Once verified, the modal shows the status and lets you revoke verification; it does not offer another verification request. On acceptance, both devices verify the account key authenticated by the live peer connection. Trust is saved in encrypted local chat storage and does not sync through the account API.

A verified key change is marked and blocks messaging, attachments, and calls until the friend accepts a new live verification request. Group membership also shows identity status. MLS credentials are signed by the account identity; a changed credential for an existing group member is accepted only when that member's account identity is verified, and the change is shown in group membership.

Live acceptance proves that the connected friend controls the account key presented by the account directory. It does not independently prove who that person is, and a malicious server could still associate the wrong key with a username. To detect that, compare fingerprints through a trusted out-of-band channel. Resonance has no independent transparency log or external witnesses, so it cannot detect a server that consistently presents different keys to different people unless they compare fingerprints independently.

Current clients do not model stable per-device identities: peer transport and MLS signing credentials derive from the account identity. MLS key packages contain one-time encryption keys, not a stable device identifier. Thus the client can validate and surface MLS credential changes, but cannot attribute one to a named device or provide separate per-device trust approval.

## Voice calls

Start an audio call from a conversation with the phone button.

Audio travels directly between participants over WebRTC and is encrypted in transit with DTLS-SRTP. Group calls use a peer-to-peer mesh; media is not relayed through the account server. Call signaling uses the configured account API and remains end-to-end encrypted, though the service can see routing metadata. Resonance uses public STUN for connection setup but does not configure TURN or a media relay, so restrictive NAT, carrier-grade NAT, or firewall rules may prevent a call. Calls need an available microphone and audio output device.

## Send messages and manage conversations

Messages to offline recipients are queued in encrypted form and delivered when they next connect. The server records message send, delivery, and read timestamps, but cannot read message contents.

Recent conversations appear in the sidebar, newest first. Closing a conversation removes it from the list; open it again from Friends to restore it. Deleting a conversation removes its local history and returns to Home. Deleting one of your messages replaces it with a deletion notice on your device only; it does not retract a message already delivered or queued for your friend.

## Create or join a group

Open **Groups**, select **Create a group**, enter a name, and choose at least two friends. Groups currently support up to 12 members, including the creator.

Invitations are encrypted and queued for each selected friend. The group appears for them when their client next checks for messages. All participants need a compatible updated client and API. Group messages are encrypted separately for each member and queued while they are offline. Members can message one another in the group without adding every member as a direct friend.

Group membership cannot currently be changed or removed after creation. Deleting a local conversation does not remove your account from the server-side group.

## Privacy and account safety

- Use a unique, strong password. Do not share it or your invitation code.
- Your password unlocks your identity key. Losing access to that identity may make local history unreadable.
- Chat history is encrypted and stored on the device where it was created. Back it up if you need to keep it.
- The server stores profile and account data, encrypted identity data, group membership metadata, and temporarily queued encrypted message envelopes. It cannot read message contents.
- Only download installers from a source you trust.

## Related documentation

- [Resonance overview](../README.md)
- [Local Development](DEVELOPMENT.md)
- [Server Deployment](DEPLOYMENT.md)
