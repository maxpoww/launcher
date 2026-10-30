// wakeload: how late a sleeper wakes while every CPU is busy with ordinary work. Half the children burn in user space, half in the kernel (small writes+reads through a pipe).
#define _GNU_SOURCE
#include <stdio.h>
#include <stdlib.h>
#include <stdint.h>
#include <time.h>
#include <unistd.h>
#include <signal.h>
#include <sys/wait.h>
static double now(void){struct timespec t;clock_gettime(CLOCK_MONOTONIC,&t);return t.tv_sec+t.tv_nsec*1e-9;}
static int cmp(const void*a,const void*b){double x=*(double*)a,y=*(double*)b;return x<y?-1:x>y;}
static double v[20000];
int main(){int n=sysconf(_SC_NPROCESSORS_ONLN);pid_t k[256];
 for(int i=0;i<n;i++){k[i]=fork();if(!k[i]){if(i%2){volatile uint64_t x=1;for(;;){x=x*3+1;}}else{int p[2];pipe(p);char b[4096]={0};for(;;){write(p[1],b,4096);read(p[0],b,4096);}}}}
 sleep(2);
 for(int r=0;r<8000;r++){struct timespec s={0,1000000};double t=now();nanosleep(&s,0);v[r]=now()-t-0.001;}
 qsort(v,8000,sizeof(double),cmp);printf("wake late, all %d CPUs busy: median %8.1f us  p95 %8.1f  p99 %8.1f  p99.9 %8.1f  max %8.1f\n",n,v[4000]*1e6,v[7600]*1e6,v[7920]*1e6,v[7992]*1e6,v[7999]*1e6);
 for(int i=0;i<n;i++)kill(k[i],SIGKILL);while(wait(0)>0);return 0;}
