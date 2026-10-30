// burst: the shape of an interactive task. Sleep (idle), then a fixed small job; how long does the job take from waking?
// wake: how late a 1 ms sleep comes back. single/multi: throughput.
#define _GNU_SOURCE
#include <stdio.h>
#include <stdlib.h>
#include <stdint.h>
#include <string.h>
#include <time.h>
#include <unistd.h>
#include <sys/wait.h>
static double now(void){struct timespec t;clock_gettime(CLOCK_MONOTONIC,&t);return t.tv_sec+t.tv_nsec*1e-9;}
static volatile uint64_t sink;
static uint64_t work(uint64_t n){uint64_t x=88172645463325252ULL;for(uint64_t i=0;i<n;i++){x^=x<<13;x^=x>>7;x^=x<<17;}sink=x;return x;}
static int cmp(const void*a,const void*b){double x=*(double*)a,y=*(double*)b;return x<y?-1:x>y;}
static void report(const char*name,double*v,int n,const char*u,double scale){qsort(v,n,sizeof(double),cmp);printf("%-22s median %8.3f %s  p95 %8.3f  p99 %8.3f  max %8.3f\n",name,v[n/2]*scale,u,v[n*95/100]*scale,v[n*99/100]*scale,v[n-1]*scale);}
int main(int argc,char**argv){
  int rounds=400; static double v[20000];
  // burst after idle: 200 ms asleep, then a job of ~5 ms at full speed
  for(int r=0;r<rounds;r++){struct timespec s={0,200000000};nanosleep(&s,0);double t=now();work(12000000);v[r]=now()-t;}
  report("burst 200ms idle+job",v,rounds,"ms",1e3);
  // long idle then job
  for(int r=0;r<20;r++){sleep(1);double t=now();work(12000000);v[r]=now()-t;}
  report("burst 1s idle+job",v,20,"ms",1e3);
  // wake latency
  for(int r=0;r<5000;r++){struct timespec s={0,1000000};double t=now();nanosleep(&s,0);v[r]=now()-t-0.001;}
  report("wake late (1 ms sleep)",v,5000,"us",1e6);
  // single throughput
  for(int r=0;r<5;r++){double t=now();work(2000000000ULL);v[r]=now()-t;}
  report("single 2e9 steps",v,5,"s",1);
  // all threads
  int n=sysconf(_SC_NPROCESSORS_ONLN);
  for(int r=0;r<3;r++){double t=now();for(int i=0;i<n;i++){if(!fork()){work(3000000000ULL);_exit(0);}}while(wait(0)>0);v[r]=now()-t;}
  report("multi all threads",v,3,"s",1);
  return 0;
}
